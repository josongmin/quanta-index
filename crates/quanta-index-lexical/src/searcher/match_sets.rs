//! Regex, phrase and raw-substring match sets over the text authority.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::phrase::{PhraseField, PhrasePolicy, plan_phrase};
use crate::query_errors::{map_phrase_plan_error, map_positions_error, map_trigram_error};
use crate::regex_match_cache::{RegexMatchCacheKey, RegexMatchCacheRefusal};
use crate::searcher::authorities::TextAuthorityFeature;
use crate::searcher::planner_errors::map_regex_plan_error;
use crate::{GenKey, TantivySearcher, normalize};
use quanta_index_contract::LqOptions;
use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_regex::RegexExecutor;
#[cfg(test)]
use quanta_index_lq_trigram::{DocId as TrigramDocId, MAX_CANDIDATE_PRE_VERIFY};
use quanta_index_lq_trigram::{LimitDimension, TrigramError};
use roaring::RoaringTreemap;
use std::sync::Arc;

use crate::searcher::preview_types::{
    PreviewResult, PreviewStop, SelectedSnippetSource, SnippetContext, integrity,
    token_allocation_bound,
};
use core::ops::Range;
use quanta_index_contract::{LqExpr, LqLeaf, LqPatternType, PreviewUnavailableReason};
use quanta_index_lq_regex::executor::RegexRangeError;
use quanta_index_lq_text_normalizer::MappedText;
#[cfg(test)]
use quanta_index_lq_trigram::TrigramPostingSource;
use std::cell::Cell;
#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// Preserve the union's candidate cap per intersection, including regex alternatives.
#[cfg(test)]
struct CountingTrigramSource<'a, S> {
    source: &'a S,
    counts: &'a RefCell<BTreeMap<Vec<[u8; 3]>, usize>>,
    seen: RefCell<BTreeSet<Vec<[u8; 3]>>>,
}
#[cfg(test)]
impl<S: TrigramPostingSource> TrigramPostingSource for CountingTrigramSource<'_, S> {
    fn intersect_trigrams(&self, grams: &[[u8; 3]]) -> Result<Vec<TrigramDocId>, TrigramError> {
        let part = self.source.intersect_trigrams(grams)?;
        let first_in_shard = self.seen.borrow_mut().insert(grams.to_vec());
        let mut counts = self.counts.borrow_mut();
        let count = counts.entry(grams.to_vec()).or_default();
        if first_in_shard {
            *count = count.saturating_add(part.len());
        }
        if *count > MAX_CANDIDATE_PRE_VERIFY {
            return Err(TrigramError::plan_limit(
                LimitDimension::CandidateSet,
                format!(
                    "candidate set across shards {count} exceeds cap {MAX_CANDIDATE_PRE_VERIFY}"
                ),
            ));
        }
        Ok(part)
    }
}

/// Verify-only regexes have the same pre-verify candidate cap as a usable
/// trigram prefilter.
///
/// Observe cancellation before advancing the authority
/// iterator and refuse an oversized set before allocating its next slot.
#[cfg(test)]
fn bounded_verify_only_candidates(
    mut doc_ids: impl Iterator<Item = u64>,
    budget: &RequestBudgetV1,
) -> Result<Vec<TrigramDocId>, CoreError> {
    let mut out = Vec::new();
    loop {
        budget.checkpoint("lexical:regex-verify-only-candidates")?;
        let Some(doc_id) = doc_ids.next() else {
            return Ok(out);
        };
        if out.len() >= MAX_CANDIDATE_PRE_VERIFY {
            let error = TrigramError::plan_limit(
                LimitDimension::CandidateSet,
                format!("verify-only regex candidate set exceeds cap {MAX_CANDIDATE_PRE_VERIFY}"),
            );
            return Err(map_trigram_error("regex verify-only prefilter", &error));
        }
        out.try_reserve(1).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: regex candidate allocation refused: {error}"
            ))
        })?;
        out.push(TrigramDocId(doc_id));
    }
}

impl TantivySearcher {
    /// The text documents containing `needle` as bytes, as authority doc
    /// ids.
    ///
    /// The verify pass resolves every candidate through the authority and
    /// refuses one it cannot resolve, so every id here names a document.
    pub(crate) fn raw_substring_match_set(
        &self,
        needle: &str,
        options: &LqOptions,
        budget: &RequestBudgetV1,
        native: &super::authorities::NativeQueryScope,
    ) -> Result<RoaringTreemap, CoreError> {
        let authority = native.authority(TextAuthorityFeature::RawSubstring)?;
        let folded = !Self::is_case_sensitive(options);
        let needle = normalize::nfc(needle);
        let query_bytes = normalize::apply_case(needle.as_ref(), Self::case_mode(options))
            .into_owned()
            .into_bytes();
        let mut out = RoaringTreemap::new();
        if query_bytes.is_empty() {
            return Ok(out);
        }
        if query_bytes.len() < 3 {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexRawSubstringTrigramIndexMissing,
                message: "lexical: raw substring shorter than three bytes requires a verify-only route".into(),
            });
        }
        let mut grams: Vec<[u8; 3]> = quanta_index_lq_trigram::trigrams_of(&query_bytes).collect();
        grams.sort_unstable();
        grams.dedup();
        if grams.len() > quanta_index_lq_trigram::MAX_TRIGRAMS_PER_QUERY {
            return Err(map_trigram_error(
                "raw substring prefilter",
                &TrigramError::plan_limit(
                    LimitDimension::Trigrams,
                    format!(
                        "needle yields {} distinct trigrams (cap {})",
                        grams.len(),
                        quanta_index_lq_trigram::MAX_TRIGRAMS_PER_QUERY,
                    ),
                ),
            ));
        }
        for id in authority.intersect_grams(&grams, folded, budget)? {
            budget.checkpoint("lexical:raw-substring-verify")?;
            let document = authority.read_document(id, budget)?.ok_or_else(|| {
                CoreError::Storage(format!(
                    "lexical: raw substring candidate {id} has no current Native body"
                ))
            })?;
            let bytes = if folded {
                document.folded_text.as_bytes()
            } else {
                document.indexed_text.as_bytes()
            };
            if memchr::memmem::find(bytes, &query_bytes).is_some() {
                let _inserted = out.insert(id);
            }
        }
        budget.checkpoint("lexical:raw-substring-complete")?;
        Ok(out)
    }

    /// The regex source as executed.
    ///
    /// NFC-normalized as text (the documents are NFC, so a decomposed literal
    /// could never match), with the engine's own `(?i)` under `case:no`.
    pub(crate) fn regex_source_for_options(source: &str, options: &LqOptions) -> String {
        let source = normalize::nfc(source);
        if Self::is_case_sensitive(options) {
            return source.into_owned();
        }
        format!("(?i){source}")
    }

    pub(crate) fn regex_match_cache_key(&self, normalized_source: &str) -> RegexMatchCacheKey {
        RegexMatchCacheKey {
            generation: GenKey {
                repo_id: self.repo_id.clone(),
                revision_id: self.revision_id.clone(),
                generation: self.generation,
            },
            normalized_source: normalized_source.to_string(),
        }
    }

    /// The text documents `source` matches, as authority doc ids, from the
    /// match cache when the regex carries no timeout.
    ///
    /// The verify pass resolves every prefiltered candidate through the
    /// authority and refuses one it cannot resolve, so every id here names
    /// a document. A set computed here is accounted in the cache's stats
    /// whether or not it is then kept.
    pub(crate) fn regex_match_set(
        &self,
        source: &str,
        options: &LqOptions,
        budget: &RequestBudgetV1,
        native: &super::authorities::NativeQueryScope,
    ) -> Result<Arc<RoaringTreemap>, CoreError> {
        let normalized_source = Self::regex_source_for_options(source, options);
        let cache_key = options
            .timeout_ms
            .is_none()
            .then(|| self.regex_match_cache_key(&normalized_source));
        if let Some(cache_key) = cache_key.as_ref() {
            let mut cache = self.regex_match_cache.lock().map_err(|err| {
                CoreError::Storage(format!("lexical regex cache poisoned: {err}"))
            })?;
            if let Some(cached) = cache.get(cache_key) {
                budget.checkpoint("lexical:regex-cache-hit")?;
                return Ok(cached);
            }
        }
        let authority = native.authority(TextAuthorityFeature::RegexTrigram)?;
        let (plan, prepared) =
            crate::regex::plan_regex_prepared(&normalized_source, options, &self.regex_policy)
                .map_err(map_regex_plan_error)?;
        let executor =
            RegexExecutor::compile_prepared(prepared).map_err(|err| CoreError::Typed {
                code: crate::query_errors::regex_wire_code(err.code),
                message: format!("lexical: regex execution compile for {source:?}: {err}"),
            })?;
        let candidates = authority.regex_candidates(plan.necessary_gram_expr(), budget)?;
        let timeout = options.timeout_ms.map(Duration::from_millis);
        let mut verification_elapsed = Duration::ZERO;
        let mut out = RoaringTreemap::new();
        for id in candidates {
            budget.checkpoint("lexical:regex-verify")?;
            if timeout.is_some_and(|limit| limit.is_zero() || verification_elapsed >= limit) {
                return Err(CoreError::Typed {
                    code: LexicalErrorCode::QueryTimeout.into(),
                    message: "lexical: regex verify timed out before candidate verification".into(),
                });
            }
            let document = authority.read_document(id, budget)?.ok_or_else(|| {
                CoreError::Storage(format!(
                    "lexical: regex candidate {id} has no current Native body"
                ))
            })?;
            let started = Instant::now();
            let matched = executor.verify(document.indexed_text.as_bytes());
            verification_elapsed = verification_elapsed
                .checked_add(started.elapsed())
                .ok_or_else(|| {
                    CoreError::Storage("lexical: regex verify duration overflow".into())
                })?;
            if timeout.is_some_and(|limit| verification_elapsed > limit) {
                return Err(CoreError::Typed {
                    code: LexicalErrorCode::QueryTimeout.into(),
                    message: "lexical: regex cumulative verification timed out".into(),
                });
            }
            budget.checkpoint("lexical:regex-candidate-verified")?;
            if matched {
                let _inserted = out.insert(id);
            }
        }
        budget.checkpoint("lexical:regex-verify-complete")?;
        let out = Arc::new(out);
        let mut cache = self
            .regex_match_cache
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical regex cache poisoned: {err}")))?;
        cache.record_built(&out);
        if let Some(cache_key) = cache_key {
            let _kept: Result<(), RegexMatchCacheRefusal> =
                cache.insert(cache_key, Arc::clone(&out));
        }
        drop(cache);
        Ok(out)
    }

    /// The text documents holding the phrase, as authority doc ids.
    ///
    /// The phrase is matched on token positions alone, so each matched id
    /// is checked against the authority's doc table: an id the positions
    /// name but the table does not hold is a corrupt authority.
    pub(crate) fn phrase_match_set(
        &self,
        text: &str,
        options: &LqOptions,
        budget: &RequestBudgetV1,
        native: &super::authorities::NativeQueryScope,
    ) -> Result<RoaringTreemap, CoreError> {
        let authority = native.authority(TextAuthorityFeature::PhrasePositions)?;
        let plan = plan_phrase(
            text,
            options,
            &PhrasePolicy::defaults(),
            PhraseField::Content,
        )
        .map_err(map_phrase_plan_error)?;
        if plan.tokens.len()
            > usize::try_from(quanta_index_lq_positions::MAX_PHRASE_LEN)
                .map_or(usize::MAX, |count| count)
        {
            return Err(map_positions_error(
                "phrase query",
                &quanta_index_lq_positions::PositionsError::plan_limit_exceeded(
                    quanta_index_lq_positions::LimitDimension::PhraseLen,
                    "phrase exceeds position-engine length cap",
                ),
            ));
        }
        let terms = plan.tokens.iter().map(String::as_str).collect::<Vec<_>>();
        let mut unique_terms = terms.clone();
        unique_terms.sort_unstable();
        unique_terms.dedup();
        let mut out = RoaringTreemap::new();
        for id in authority.intersect_terms(&unique_terms, plan.case_sensitive, budget)? {
            budget.checkpoint("lexical:phrase-verify")?;
            let document = authority.read_document(id, budget)?.ok_or_else(|| {
                CoreError::Storage(format!(
                    "lexical: phrase candidate {id} has no current Native body"
                ))
            })?;
            if phrase_contains(&document.indexed_text, &terms, plan.case_sensitive)? {
                let _inserted = out.insert(id);
            }
        }
        budget.checkpoint("lexical:phrase-complete")?;
        Ok(out)
    }
}

/// KMP over indexable tokens, resetting across skipped overlong tokens.
fn phrase_contains(indexed: &str, terms: &[&str], case_sensitive: bool) -> Result<bool, CoreError> {
    let invalid_state =
        || CoreError::InvalidContract("phrase prefix state exceeds its pattern".into());
    let term_at = |index| terms.get(index).copied().ok_or_else(invalid_state);
    if terms.is_empty() {
        return Ok(false);
    }
    let mut failure = vec![0_usize; terms.len()];
    let mut matched = 0_usize;
    for (index, expected) in terms.iter().enumerate().skip(1) {
        while matched > 0 && *expected != term_at(matched)? {
            matched = *failure
                .get(matched.saturating_sub(1))
                .ok_or_else(invalid_state)?;
        }
        if *expected == term_at(matched)? {
            matched = matched.saturating_add(1);
        }
        *failure.get_mut(index).ok_or_else(invalid_state)? = matched;
    }
    let mut matched = 0_usize;
    let mut previous_position = None;
    for token in normalize::tokenize(
        indexed,
        normalize::CaseMode::from_case_sensitive(case_sensitive),
    )
    .indexable()
    {
        if previous_position.is_some_and(|prior: usize| token.position != prior.saturating_add(1)) {
            matched = 0;
        }
        previous_position = Some(token.position);
        while matched > 0 && token.text != term_at(matched)? {
            matched = *failure
                .get(matched.saturating_sub(1))
                .ok_or_else(invalid_state)?;
        }
        if token.text == term_at(matched)? {
            matched = matched.saturating_add(1);
            if matched == terms.len() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[derive(Clone)]
pub(crate) struct PositiveWitness {
    pub(crate) original: Range<usize>,
    pub(crate) normalized: Range<usize>,
}

/// Reconstruct selected-row witnesses through the existing matching owners.
///
/// False Boolean branches and NOT retain no positive evidence. This request-
/// local operation never caches whole-query truth under a leaf cache key.
pub(crate) fn selected_witnesses<'a>(
    context: &SnippetContext<'_>,
    source: &SelectedSnippetSource<'_>,
    nfc: &MappedText<'_>,
    folded: &MappedText<'_>,
    document: &[normalize::Token],
    regex_executor: &dyn Fn(&str) -> Result<&'a RegexExecutor, CoreError>,
    predicate_truth: &dyn Fn(&LqLeaf) -> Result<bool, CoreError>,
) -> PreviewResult<(bool, bool, Vec<PositiveWitness>)> {
    let evaluator = WitnessMatcher {
        context,
        source,
        nfc,
        folded,
        document,
        regex_executor,
        predicate_truth,
        overflow: Cell::new(false),
        unsupported: Cell::new(false),
    };
    let mut witnesses = Vec::new();
    let include_path = TantivySearcher::enables_path_term_surface(context.expr, context.options);
    let (mut matched, path_match) =
        evaluator.evaluate(context.expr, include_path, 0, &mut witnesses)?;
    if matched {
        for filter in context.filters {
            if let quanta_index_contract::LqFilter::Content { leaf } = filter
                && !evaluator.leaf(leaf, false, &mut witnesses)?.0
            {
                matched = false;
                witnesses.clear();
                break;
            }
        }
    }
    if matched && evaluator.overflow.get() {
        return Err(PreviewStop::Unavailable(
            PreviewUnavailableReason::WorkBudget,
        ));
    }
    if matched && evaluator.unsupported.get() {
        return Err(PreviewStop::Unavailable(
            PreviewUnavailableReason::UnsupportedRange,
        ));
    }
    Ok((matched, path_match, witnesses))
}

struct WitnessMatcher<'a, 'b, 'c> {
    context: &'a SnippetContext<'b>,
    source: &'a SelectedSnippetSource<'b>,
    nfc: &'a MappedText<'b>,
    folded: &'a MappedText<'b>,
    document: &'a [normalize::Token],
    regex_executor: &'a dyn Fn(&str) -> Result<&'c RegexExecutor, CoreError>,
    predicate_truth: &'a dyn Fn(&LqLeaf) -> Result<bool, CoreError>,
    overflow: Cell<bool>,
    unsupported: Cell<bool>,
}

impl WitnessMatcher<'_, '_, '_> {
    fn evaluate(
        &self,
        expr: &LqExpr,
        include_path: bool,
        depth: usize,
        out: &mut Vec<PositiveWitness>,
    ) -> PreviewResult<(bool, bool)> {
        self.context.charge(1)?;
        if depth > 64 {
            return Err(PreviewStop::Unavailable(
                PreviewUnavailableReason::WorkBudget,
            ));
        }
        match expr {
            LqExpr::Empty => Ok((true, false)),
            LqExpr::Leaf(leaf) => self.leaf(leaf, include_path, out),
            LqExpr::Not(inner) => {
                let saved = out.len();
                let flags = (self.overflow.get(), self.unsupported.get());
                let result = self.evaluate(inner, false, depth.saturating_add(1), out);
                out.truncate(saved);
                self.overflow.set(flags.0);
                self.unsupported.set(flags.1);
                result.map(|(matched, _)| (!matched, false))
            }
            LqExpr::All(children) => {
                let saved = out.len();
                let flags = (self.overflow.get(), self.unsupported.get());
                for child in children {
                    if !self.evaluate(child, false, depth.saturating_add(1), out)?.0 {
                        out.truncate(saved);
                        self.overflow.set(flags.0);
                        self.unsupported.set(flags.1);
                        return Ok((false, false));
                    }
                }
                Ok((true, false))
            }
            LqExpr::Any(children) => {
                let mut matched = false;
                for child in children {
                    let saved = out.len();
                    let flags = (self.overflow.get(), self.unsupported.get());
                    if self.evaluate(child, false, depth.saturating_add(1), out)?.0 {
                        matched = true;
                    } else {
                        out.truncate(saved);
                        self.overflow.set(flags.0);
                        self.unsupported.set(flags.1);
                    }
                }
                Ok((matched, false))
            }
        }
    }

    fn push(
        &self,
        map: &MappedText<'_>,
        range: Range<usize>,
        out: &mut Vec<PositiveWitness>,
    ) -> PreviewResult<()> {
        if range.is_empty() || map.text().get(range.clone()).is_none() {
            self.unsupported.set(true);
            return Ok(());
        }
        if out.len() >= self.context.limits.witnesses {
            self.overflow.set(true);
            return Ok(());
        }
        let original = map
            .source_range(range.clone())
            .map_err(|error| self.context.mapping_error(error))?;
        let normalized = map
            .normalized_range(range)
            .map_err(|error| self.context.mapping_error(error))?;
        out.push(PositiveWitness {
            original,
            normalized,
        });
        Ok(())
    }

    fn gather(
        &self,
        map: &MappedText<'_>,
        ranges: impl Iterator<Item = Range<usize>>,
        out: &mut Vec<PositiveWitness>,
    ) -> PreviewResult<bool> {
        // One extra range proves an incomplete witness set and declines the
        // optional preview. Never emit a partial highlight list as complete.
        let cap = self
            .context
            .limits
            .witnesses
            .saturating_sub(out.len())
            .saturating_add(1);
        let mut matched = false;
        for range in ranges.take(cap) {
            matched = true;
            self.push(map, range, out)?;
            if self.overflow.get() || self.unsupported.get() {
                break;
            }
        }
        Ok(matched)
    }

    fn leaf(
        &self,
        leaf: &LqLeaf,
        include_path: bool,
        out: &mut Vec<PositiveWitness>,
    ) -> PreviewResult<(bool, bool)> {
        let (text, regex) = match leaf {
            LqLeaf::Keyword(text) | LqLeaf::RawString(text) => (
                text,
                self.context.options.pattern_type == LqPatternType::Regexp,
            ),
            LqLeaf::Regex(text) => (text, true),
            LqLeaf::Phrase(text) => (text, false),
            LqLeaf::Predicate { .. } | LqLeaf::StructuralBlock(_) => {
                return (self.predicate_truth)(leaf)
                    .map(|matched| (matched, false))
                    .map_err(PreviewStop::Mandatory);
            }
        };
        if text.len() > self.context.limits.source_bytes {
            return Err(PreviewStop::Unavailable(
                PreviewUnavailableReason::WorkBudget,
            ));
        }
        self.context
            .charge(self.nfc.text().len().saturating_add(text.len()))?;
        let _query_memory = self.context.reserve(token_allocation_bound(text.len())?)?;
        if regex {
            let executor = (self.regex_executor)(text).map_err(PreviewStop::Mandatory)?;
            let expected =
                crate::TantivySearcher::regex_source_for_options(text, self.context.options);
            if executor.pattern() != expected {
                return Err(PreviewStop::Mandatory(integrity(
                    "regex executor has wrong pattern/case binding",
                )));
            }
            let cap = self
                .context
                .limits
                .witnesses
                .saturating_sub(out.len())
                .saturating_add(1);
            let found = executor
                .find_ranges_bounded(
                    self.nfc.text().as_bytes(),
                    self.context.limits.transformed_bytes,
                    cap,
                    &|| self.context.request.interruption().is_some(),
                )
                .map_err(|error| match error {
                    RegexRangeError::SourceByteLimit | RegexRangeError::AllocationRefused => {
                        PreviewStop::Unavailable(PreviewUnavailableReason::WorkBudget)
                    }
                    RegexRangeError::Interrupted => PreviewStop::Mandatory(
                        self.context
                            .request
                            .interrupted_at("lexical:preview-regex")
                            .unwrap_or_else(|| integrity("unobserved regex interruption")),
                    ),
                })?;
            return self
                .gather(self.nfc, found.ranges.into_iter(), out)
                .map(|matched| (matched, false));
        }
        if matches!(leaf, LqLeaf::RawString(_)) {
            return self
                .gather(self.folded, self.folded.find_substrings(text), out)
                .map(|matched| (matched, false));
        }
        let wanted =
            normalize::query_tokens(text, self.context.options.case_mode()).map_err(|error| {
                PreviewStop::Mandatory(integrity(&format!("prepared token query invalid: {error}")))
            })?;
        self.context
            .charge(self.document.len().saturating_mul(wanted.len()))?;
        if self.gather(
            self.nfc,
            normalize::phrase_ranges(self.document, &wanted),
            out,
        )? {
            return Ok((true, false));
        }
        if include_path && matches!(leaf, LqLeaf::Keyword(_)) {
            if self.source.path.len() > self.context.limits.source_bytes {
                return Err(PreviewStop::Unavailable(
                    PreviewUnavailableReason::WorkBudget,
                ));
            }
            self.context.charge(self.source.path.len())?;
            let _path_memory = self
                .context
                .reserve(token_allocation_bound(self.source.path.len())?)?;
            let path = normalize::tokenize(self.source.path, self.context.options.case_mode());
            let present: Vec<_> = path.indexable().cloned().collect();
            self.context
                .charge(present.len().saturating_mul(wanted.len()))?;
            return Ok((normalize::contains_phrase(&present, &wanted), true));
        }
        Ok((false, false))
    }
}

#[cfg(test)]
mod l4_verify_only_candidate_bounds {
    use super::{MAX_CANDIDATE_PRE_VERIFY, bounded_verify_only_candidates};
    use quanta_index_contract::SearchPlaneErrorCodeV2;
    use quanta_index_core::{CoreError, RequestBudgetV1};
    use std::cell::Cell;

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "fixed regression fixture assertions"
    )]
    fn fallback_preserves_the_candidate_cap_and_checks_cancellation_before_enumeration()
    -> Result<(), CoreError> {
        let max = u64::try_from(MAX_CANDIDATE_PRE_VERIFY)
            .map_err(|error| CoreError::Storage(error.to_string()))?;
        let budget = RequestBudgetV1::unbounded();
        let admitted = bounded_verify_only_candidates(1..=max, &budget)?;
        assert_eq!(admitted.len(), MAX_CANDIDATE_PRE_VERIFY);
        let oversized = bounded_verify_only_candidates(1..=max.saturating_add(1), &budget);
        assert!(matches!(
            oversized,
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            })
        ));

        let cancelled = RequestBudgetV1::unbounded();
        cancelled.cancel_handle().cancel();
        let advanced = Cell::new(false);
        let interrupted = bounded_verify_only_candidates(
            std::iter::from_fn(|| {
                advanced.set(true);
                Some(1)
            }),
            &cancelled,
        );
        assert!(interrupted.is_err());
        assert!(!advanced.get());
        Ok(())
    }
}

#[cfg(test)]
mod paging_limit_tests {
    use super::{BTreeMap, BTreeSet, CountingTrigramSource, RefCell};
    use quanta_index_lq_trigram::{
        DocId, MAX_CANDIDATE_PRE_VERIFY, TrigramError, TrigramErrorCode, TrigramPostingSource,
    };
    struct FixedCandidates(usize);
    impl TrigramPostingSource for FixedCandidates {
        fn intersect_trigrams(&self, _grams: &[[u8; 3]]) -> Result<Vec<DocId>, TrigramError> {
            (0..self.0)
                .map(|value| {
                    u64::try_from(value).map(DocId).map_err(|error| {
                        TrigramError::new(
                            TrigramErrorCode::IndexCorrupted,
                            format!("fixed candidate id overflow: {error}"),
                        )
                    })
                })
                .collect()
        }
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "independent fixed candidate cap assertions"
    )]
    fn candidate_cap_is_aggregate_and_repeated_alternative_is_charged_once_per_shard()
    -> Result<(), Box<dyn std::error::Error>> {
        let counts = RefCell::new(BTreeMap::new());
        assert_eq!(MAX_CANDIDATE_PRE_VERIFY, 100_000);
        let half = FixedCandidates(50_000);
        for _ in 0..2 {
            let source = CountingTrigramSource {
                source: &half,
                counts: &counts,
                seen: RefCell::new(BTreeSet::new()),
            };
            assert_eq!(source.intersect_trigrams(&[*b"abc"])?.len(), half.0);
            assert_eq!(source.intersect_trigrams(&[*b"abc"])?.len(), half.0);
        }
        let one = FixedCandidates(1);
        let source = CountingTrigramSource {
            source: &one,
            counts: &counts,
            seen: RefCell::new(BTreeSet::new()),
        };
        let error = source
            .intersect_trigrams(&[*b"abc"])
            .expect_err("union above the unchanged cap must refuse");
        assert_eq!(error.code, TrigramErrorCode::PlanLimitExceeded);
        assert_eq!(
            error.dimension,
            Some(quanta_index_lq_trigram::LimitDimension::CandidateSet)
        );
        Ok(())
    }
}
