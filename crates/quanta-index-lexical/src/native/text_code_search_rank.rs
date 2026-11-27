//! Legacy `CodeSearch` file ranking over verified Native Source bytes.
//!
//! A port of the Legacy selected-file scorer (`searcher/code_search.rs`):
//! per literal or regex term the best boundary witness over the term's
//! surfaces (Source body and repository-relative path), its repeated
//! occurrences, the folded-query exact-case preference of a literal, and the
//! minimum-cover proximity of all-literal terms on one surface. The score is a pure function of the verified Source
//! text, the authenticated path and the query terms; it reads no index state.
//! Ties between files keep the page order (score, repository, path).

use core::ops::Range;
use std::borrow::Cow;

use quanta_index_core::index::{IndexError, MemoryLifetime, WorkBudget, WorkUsage};
use quanta_index_lq_regex::RegexExecutor;
use quanta_index_lq_regex::executor::RegexRangeError;

use super::text::CaseMode;
use crate::normalize::{self, MappedText, MappingError};

/// Legacy refuses a regex with more matches than this on one surface.
const MAX_REGEX_MATCHES_PER_SURFACE: usize = 4_096;

/// Which surfaces one term may match, as in the Legacy plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TermScope {
    /// An unscoped term: the Source body or the path.
    Both,
    /// `content:`: the Source body only.
    Content,
    /// `path:`: the repository-relative path only.
    Path,
}

/// How one query term matches, as given to [`CodeSearchRank::new`].
#[derive(Clone, Copy)]
pub(super) enum RankMatcher<'e> {
    /// A literal in its query spelling.
    Literal(&'e str),
    /// A regex: the engine for each surface its scope searches. Both run on
    /// NFC text with the query's case mode compiled into the pattern.
    Regex {
        content: Option<&'e RegexExecutor>,
        path: Option<&'e RegexExecutor>,
    },
}

/// One term's prepared matcher.
pub(super) enum TermMatch<'e> {
    /// The NFC spelling and the case-mode needle.
    Literal { text: String, needle: String },
    Regex {
        content: Option<&'e RegexExecutor>,
        path: Option<&'e RegexExecutor>,
    },
}

/// One query term.
pub(super) struct RankTerm<'e> {
    matcher: TermMatch<'e>,
    scope: TermScope,
}

/// The literal and regex terms of one CodeSearch-shaped query, in query order.
pub(super) struct CodeSearchRank<'e> {
    terms: Vec<RankTerm<'e>>,
    case: CaseMode,
}

/// The Legacy additive score components of one file.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CodeSearchScore {
    pub boundary_and_path: u32,
    pub occurrence: u32,
    pub exact_case: u32,
    pub proximity: u32,
}

impl CodeSearchScore {
    #[must_use]
    pub const fn total(self) -> u32 {
        self.boundary_and_path
            .saturating_add(self.occurrence)
            .saturating_add(self.exact_case)
            .saturating_add(self.proximity)
    }
}

/// The representative verified match of one ranked file.
///
/// It is the Legacy primary witness: the term witness with the highest
/// boundary score, the earlier term on a tie, and within one term the first
/// occurrence with that score; a path witness replaces a body witness of the
/// same term only when it scores strictly higher. Byte offsets index the
/// verified original bytes of the surface, never their NFC or folded form.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodeSearchMatch {
    /// Source body bytes `start..end` and the 1-based lines of their first
    /// and last byte, counted on the verified Source bytes.
    Content {
        start: u64,
        end: u64,
        start_line: u32,
        end_line: u32,
    },
    /// Repository-relative path bytes `start..end`.
    Path { start: u64, end: u64 },
}

/// One file's Legacy score and representative match.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RankedFile {
    pub(super) score: CodeSearchScore,
    pub(super) matched: CodeSearchMatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HitSurface {
    Content,
    Path,
}

#[derive(Clone, Debug)]
struct Witness {
    surface: HitSurface,
    /// A span of the surface's case-mode text (`mapping_case` folded) or of
    /// its NFC text (`mapping_case` sensitive, as for every regex).
    normalized: Range<usize>,
    score: u32,
    occurrences: u8,
    mapping_case: CaseMode,
}

/// The raw, NFC and case-mode forms of one verified surface.
struct Surface<'a> {
    raw: &'a str,
    indexed: Cow<'a, str>,
    scanned: Cow<'a, str>,
}

impl<'a> Surface<'a> {
    fn prepare(raw: &'a str, case: CaseMode, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        charge_bytes(budget, raw.len())?;
        let indexed = normalize::nfc(raw);
        let scanned = match &indexed {
            Cow::Borrowed(text) => normalize::apply_case(text, case),
            Cow::Owned(text) => Cow::Owned(normalize::apply_case(text, case).into_owned()),
        };
        charge_bytes(budget, indexed.len().saturating_add(scanned.len()))?;
        Ok(Self {
            raw,
            indexed,
            scanned,
        })
    }
}

fn charge_bytes(budget: &mut WorkBudget, bytes: usize) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        bytes: u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })
}

impl<'e> CodeSearchRank<'e> {
    pub(super) fn new(
        terms: impl IntoIterator<Item = (RankMatcher<'e>, TermScope)>,
        case: CaseMode,
    ) -> Self {
        let terms = terms
            .into_iter()
            .map(|(matcher, scope)| RankTerm {
                matcher: match matcher {
                    RankMatcher::Literal(raw) => {
                        let text = normalize::nfc(raw).into_owned();
                        TermMatch::Literal {
                            needle: normalize::apply_case(&text, case).into_owned(),
                            text,
                        }
                    }
                    RankMatcher::Regex { content, path } => TermMatch::Regex { content, path },
                },
                scope,
            })
            .collect();
        Self { terms, case }
    }

    /// Whether any term may match the Source body.
    pub(super) fn reads_content(&self) -> bool {
        self.terms
            .iter()
            .any(|term| !matches!(term.scope, TermScope::Path))
    }

    /// Score one verified file; `None` when some term has no witness.
    ///
    /// `content` is the verified UTF-8 Source body; it is `None` only when
    /// every term is path-scoped.
    pub(super) fn score(
        &self,
        content: Option<&str>,
        path: &str,
        budget: &mut WorkBudget,
    ) -> Result<Option<RankedFile>, IndexError> {
        if content.is_none() && self.reads_content() {
            return Err(IndexError::BackendViolation(
                "code search body term lacks verified Source text".into(),
            ));
        }
        budget.checkpoint()?;
        let content = content
            .map(|text| Surface::prepare(text, self.case, budget))
            .transpose()?;
        let path = Surface::prepare(path, self.case, budget)?;
        let mut components = CodeSearchScore::default();
        let mut primary: Option<(Witness, usize)> = None;
        for (index, term) in self.terms.iter().enumerate() {
            let Some(witness) = self.choose_witness(content.as_ref(), &path, term, budget)? else {
                return Ok(None);
            };
            let surface = match witness.surface {
                HitSurface::Content => content.as_ref().ok_or_else(|| {
                    IndexError::BackendViolation("code search body witness lacks text".into())
                })?,
                HitSurface::Path => &path,
            };
            let case_exact = match &term.matcher {
                TermMatch::Literal { text, .. } => {
                    self.case == CaseMode::Folded && surface.indexed.contains(text.as_str())
                }
                TermMatch::Regex { .. } => false,
            };
            components.boundary_and_path =
                components.boundary_and_path.saturating_add(witness.score);
            components.occurrence = components
                .occurrence
                .saturating_add(u32::from(witness.occurrences.saturating_sub(1)).saturating_mul(2));
            components.exact_case =
                components
                    .exact_case
                    .saturating_add(if case_exact { 5 } else { 0 });
            if primary.as_ref().is_none_or(|(prior, prior_term)| {
                witness.score > prior.score || (witness.score == prior.score && index < *prior_term)
            }) {
                primary = Some((witness, index));
            }
        }
        let Some((primary, _term)) = primary else {
            return Ok(None);
        };
        components.proximity = self.proximity(content.as_ref(), &path, budget)?;
        if u16::try_from(components.total()).is_err() {
            return Err(IndexError::BackendViolation(
                "code search score exceeds the Legacy score range".into(),
            ));
        }
        let matched = match primary.surface {
            HitSurface::Content => {
                let surface = content.as_ref().ok_or_else(|| {
                    IndexError::BackendViolation("code search body witness lacks text".into())
                })?;
                let original =
                    original_range(surface, &primary.normalized, primary.mapping_case, budget)?;
                let (start_line, end_line) = lines(surface.raw, &original, budget)?;
                CodeSearchMatch::Content {
                    start: offset(original.start)?,
                    end: offset(original.end)?,
                    start_line,
                    end_line,
                }
            }
            HitSurface::Path => {
                let original =
                    original_range(&path, &primary.normalized, primary.mapping_case, budget)?;
                CodeSearchMatch::Path {
                    start: offset(original.start)?,
                    end: offset(original.end)?,
                }
            }
        };
        Ok(Some(RankedFile {
            score: components,
            matched,
        }))
    }

    fn choose_witness(
        &self,
        content: Option<&Surface<'_>>,
        path: &Surface<'_>,
        term: &RankTerm<'e>,
        budget: &mut WorkBudget,
    ) -> Result<Option<Witness>, IndexError> {
        let reads_content = matches!(term.scope, TermScope::Both | TermScope::Content);
        let reads_path = matches!(term.scope, TermScope::Both | TermScope::Path);
        let (mut best, path_match) = match &term.matcher {
            TermMatch::Literal { needle, .. } => (
                match content.filter(|_| reads_content) {
                    Some(content) => best_in(
                        &content.scanned,
                        needle,
                        HitSurface::Content,
                        self.case,
                        budget,
                    )?,
                    None => None,
                },
                if reads_path {
                    best_in(&path.scanned, needle, HitSurface::Path, self.case, budget)?
                } else {
                    None
                },
            ),
            TermMatch::Regex {
                content: content_regex,
                path: path_regex,
            } => {
                let engine = |regex: Option<&'e RegexExecutor>| {
                    regex.ok_or_else(|| {
                        IndexError::BackendViolation(
                            "code search regex term lacks its surface engine".into(),
                        )
                    })
                };
                (
                    match content.filter(|_| reads_content) {
                        Some(content) => best_regex_in(
                            &content.indexed,
                            engine(*content_regex)?,
                            HitSurface::Content,
                            budget,
                        )?,
                        None => None,
                    },
                    if reads_path {
                        best_regex_in(
                            &path.indexed,
                            engine(*path_regex)?,
                            HitSurface::Path,
                            budget,
                        )?
                    } else {
                        None
                    },
                )
            }
        };
        if let Some(path_match) = path_match
            && best
                .as_ref()
                .is_none_or(|prior| path_match.score > prior.score)
        {
            best = Some(path_match);
        }
        Ok(best)
    }

    fn proximity(
        &self,
        content: Option<&Surface<'_>>,
        path: &Surface<'_>,
        budget: &mut WorkBudget,
    ) -> Result<u32, IndexError> {
        let mut best = match content {
            Some(content) => {
                min_cover_gap(&content.scanned, &self.terms, HitSurface::Content, budget)?
            }
            None => None,
        };
        if let Some(path_gap) = min_cover_gap(&path.scanned, &self.terms, HitSurface::Path, budget)?
        {
            best = Some(best.map_or(path_gap, |gap: usize| gap.min(path_gap)));
        }
        let Some(gap) = best else {
            return Ok(0);
        };
        if gap >= 32 {
            return Ok(0);
        }
        let gap = u32::try_from(gap).map_err(|_width| IndexError::ResourceLimited)?;
        Ok(32_u32.saturating_sub(gap))
    }
}

/// The Legacy boundary score of one normalized match span.
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

/// Every literal witness, including matches that overlap a previous one.
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

/// The first best-scoring occurrence and the occurrence count, capped at 4.
fn best_in(
    text: &str,
    needle: &str,
    surface: HitSurface,
    mapping_case: CaseMode,
    budget: &mut WorkBudget,
) -> Result<Option<Witness>, IndexError> {
    charge_bytes(budget, text.len().saturating_add(needle.len()))?;
    let mut best: Option<Witness> = None;
    let mut occurrences = 0_u8;
    for (index, start) in OverlappingMatches::new(text, needle).enumerate() {
        occurrences = occurrences.saturating_add(1).min(4);
        if index.is_multiple_of(256) {
            budget.checkpoint()?;
        }
        let end = start
            .checked_add(needle.len())
            .ok_or(IndexError::ResourceLimited)?;
        let score = boundary_score(text, start..end, surface == HitSurface::Path);
        if best.as_ref().is_none_or(|prior| score > prior.score) {
            best = Some(Witness {
                surface,
                normalized: start..end,
                score,
                occurrences: 0,
                mapping_case,
            });
        }
    }
    budget.checkpoint()?;
    if let Some(best) = &mut best {
        best.occurrences = occurrences;
    }
    Ok(best)
}

/// The Legacy regex witness: every leftmost-first match on the NFC surface,
/// refused above 4,096 matches; the first best-scoring span wins and the
/// occurrence count is capped at 4.
fn best_regex_in(
    text: &str,
    regex: &RegexExecutor,
    surface: HitSurface,
    budget: &mut WorkBudget,
) -> Result<Option<Witness>, IndexError> {
    charge_bytes(budget, text.len())?;
    let observed: &WorkBudget = budget;
    let matches = regex
        .find_ranges_bounded(
            text.as_bytes(),
            text.len(),
            MAX_REGEX_MATCHES_PER_SURFACE.saturating_add(1),
            &|| observed.checkpoint().is_err(),
        )
        .map_err(|error| match error {
            RegexRangeError::Interrupted => observed
                .checkpoint()
                .err()
                .unwrap_or(IndexError::ResourceLimited),
            RegexRangeError::SourceByteLimit | RegexRangeError::AllocationRefused => {
                IndexError::ResourceLimited
            }
        })?;
    budget.checkpoint()?;
    if !matches.exhausted || matches.ranges.len() > MAX_REGEX_MATCHES_PER_SURFACE {
        return Err(IndexError::ResourceLimited);
    }
    let occurrences =
        u8::try_from(matches.ranges.len().min(4)).map_err(|_width| IndexError::ResourceLimited)?;
    let mut best: Option<Witness> = None;
    for span in matches.ranges {
        if span.is_empty() || text.get(span.clone()).is_none() {
            return Err(IndexError::InvalidInput(
                "code search regex match is zero-width or splits a UTF-8 scalar".into(),
            ));
        }
        let score = boundary_score(text, span.clone(), surface == HitSurface::Path);
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

/// The smallest byte gap of a window covering one match of every term.
///
/// `None` with fewer than two terms, when a term cannot match this surface,
/// or when a term has no match on it.
fn min_cover_gap(
    text: &str,
    terms: &[RankTerm<'_>],
    surface: HitSurface,
    budget: &mut WorkBudget,
) -> Result<Option<usize>, IndexError> {
    let mut needles: Vec<&str> = Vec::with_capacity(terms.len());
    for term in terms {
        match &term.matcher {
            TermMatch::Literal { needle, .. } => needles.push(needle),
            // Legacy proximity is defined for literal terms only.
            TermMatch::Regex { .. } => return Ok(None),
        }
    }
    if terms.len() < 2
        || terms.iter().any(|term| {
            matches!(
                (surface, term.scope),
                (HitSurface::Content, TermScope::Path) | (HitSurface::Path, TermScope::Content)
            )
        })
    {
        return Ok(None);
    }
    let mut streams: Vec<_> = needles
        .iter()
        .map(|needle| OverlappingMatches::new(text, needle))
        .collect();
    let mut current = Vec::with_capacity(streams.len());
    for (stream, needle) in streams.iter_mut().zip(&needles) {
        charge_bytes(budget, text.len().saturating_add(needle.len()))?;
        let Some(start) = stream.next() else {
            return Ok(None);
        };
        let end = start
            .checked_add(needle.len())
            .ok_or(IndexError::ResourceLimited)?;
        current.push((start, end));
    }
    let term_bytes: usize = needles.iter().map(|needle| needle.len()).sum();
    let mut smallest = usize::MAX;
    let mut steps = 0_usize;
    loop {
        if steps.is_multiple_of(256) {
            budget.checkpoint()?;
        }
        steps = steps.saturating_add(1);
        let (leftmost, &(start, _)) = current
            .iter()
            .enumerate()
            .min_by_key(|(_, (start, _))| *start)
            .ok_or_else(|| IndexError::BackendViolation("empty proximity window".into()))?;
        let end = current
            .iter()
            .map(|(_, end)| *end)
            .max()
            .ok_or_else(|| IndexError::BackendViolation("empty proximity window".into()))?;
        smallest = smallest.min(end.saturating_sub(start).saturating_sub(term_bytes));
        if smallest == 0 {
            break;
        }
        let stream = streams
            .get_mut(leftmost)
            .ok_or_else(|| IndexError::BackendViolation("proximity stream outside terms".into()))?;
        let Some(next_start) = stream.next() else {
            break;
        };
        let needle = needles
            .get(leftmost)
            .ok_or_else(|| IndexError::BackendViolation("proximity term outside terms".into()))?;
        let next_end = next_start
            .checked_add(needle.len())
            .ok_or(IndexError::ResourceLimited)?;
        let slot = current
            .get_mut(leftmost)
            .ok_or_else(|| IndexError::BackendViolation("proximity window outside terms".into()))?;
        *slot = (next_start, next_end);
    }
    budget.checkpoint()?;
    Ok(Some(smallest))
}

/// Map a span of the surface's `case` text back to its verified original
/// bytes.
///
/// NFC source only needs a scalar walk for folding offsets; other source is
/// mapped through the bounded NFC provenance map.
fn original_range(
    surface: &Surface<'_>,
    normalized: &Range<usize>,
    case: CaseMode,
    budget: &mut WorkBudget,
) -> Result<Range<usize>, IndexError> {
    let text = match case {
        CaseMode::Sensitive => &surface.indexed,
        CaseMode::Folded => &surface.scanned,
    };
    if normalized.is_empty() || text.get(normalized.clone()).is_none() {
        return Err(IndexError::BackendViolation(
            "code search witness is not UTF-8 aligned".into(),
        ));
    }
    if surface.raw == surface.indexed.as_ref() {
        if case == CaseMode::Sensitive || surface.raw.is_ascii() {
            return Ok(normalized.clone());
        }
        return folded_identity_range(surface.raw, normalized, budget);
    }
    let raw = surface.raw;
    let max_bytes = raw
        .len()
        .max(surface.indexed.len())
        .max(surface.scanned.len());
    let max_entries = raw
        .len()
        .checked_mul(8)
        .ok_or(IndexError::ResourceLimited)?
        .max(1);
    let allocation =
        MappedText::allocation_bound(max_bytes, max_entries).ok_or(IndexError::ResourceLimited)?;
    charge_bytes(budget, allocation)?;
    let _memory = budget.reserve_memory(
        u64::try_from(allocation).map_err(|_width| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let observed: &WorkBudget = budget;
    let mapped = MappedText::new(raw, &surface.indexed, case, max_bytes, max_entries, &|| {
        observed.checkpoint().is_err()
    })
    .map_err(|error| mapping_error(error, observed))?;
    mapped
        .source_range(normalized.clone())
        .map_err(|error| mapping_error(error, observed))
}

fn mapping_error(error: MappingError, budget: &WorkBudget) -> IndexError {
    match error {
        MappingError::Interrupted => budget
            .checkpoint()
            .err()
            .unwrap_or(IndexError::ResourceLimited),
        MappingError::ByteLimit | MappingError::EntryLimit | MappingError::AllocationRefused => {
            IndexError::ResourceLimited
        }
        MappingError::SourceMismatch | MappingError::InvalidSpan => IndexError::BackendViolation(
            "code search witness provenance differs from its verified source".into(),
        ),
    }
}

/// NFC source: walk scalars, measuring each one's folded length.
fn folded_identity_range(
    raw: &str,
    normalized: &Range<usize>,
    budget: &mut WorkBudget,
) -> Result<Range<usize>, IndexError> {
    charge_bytes(budget, normalized.end)?;
    let mut folded_offset = 0_usize;
    let mut original_start = None;
    let mut original_end = 0_usize;
    let mut next_checkpoint = 4096_usize;
    for (start, scalar) in raw.char_indices() {
        if start >= next_checkpoint {
            budget.checkpoint()?;
            next_checkpoint = start.saturating_add(4096);
        }
        let folded_len = if scalar.is_ascii() {
            1
        } else {
            let mut buffer = [0_u8; 4];
            normalize::apply_case(scalar.encode_utf8(&mut buffer), CaseMode::Folded).len()
        };
        let end = folded_offset
            .checked_add(folded_len)
            .ok_or(IndexError::ResourceLimited)?;
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
    let start = original_start.ok_or_else(|| {
        IndexError::BackendViolation("code search folded witness lies outside its source".into())
    })?;
    Ok(start..original_end)
}

/// 1-based lines of the first and last byte of a nonempty original span.
fn lines(
    raw: &str,
    span: &Range<usize>,
    budget: &mut WorkBudget,
) -> Result<(u32, u32), IndexError> {
    let last = span
        .end
        .checked_sub(1)
        .filter(|last| *last >= span.start)
        .ok_or_else(|| IndexError::BackendViolation("empty code search witness".into()))?;
    let bytes = raw
        .as_bytes()
        .get(..=last)
        .ok_or_else(|| IndexError::BackendViolation("code search witness outside source".into()))?;
    charge_bytes(budget, bytes.len())?;
    let newlines_before = |end: usize| -> Result<u32, IndexError> {
        let count = bytes
            .get(..end)
            .map(|prefix| memchr::memchr_iter(b'\n', prefix).count())
            .ok_or_else(|| {
                IndexError::BackendViolation("code search line outside source".into())
            })?;
        u32::try_from(count)
            .map_err(|_width| IndexError::ResourceLimited)?
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)
    };
    Ok((newlines_before(span.start)?, newlines_before(last)?))
}

fn offset(value: usize) -> Result<u64, IndexError> {
    u64::try_from(value).map_err(|_width| IndexError::ResourceLimited)
}

#[cfg(test)]
mod tests;
