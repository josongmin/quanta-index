//! Regex, phrase and raw-substring match sets over the text authority.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::budgeted_search::BudgetProbe;
use crate::phrase::{PhraseField, PhrasePolicy, plan_phrase};
use crate::query_errors::{
    fold_literal_prefix, map_phrase_plan_error, map_positions_error, map_trigram_error,
};
use crate::regex_match_cache::{RegexMatchCacheKey, RegexMatchCacheRefusal};
use crate::searcher::authorities::TextAuthorityFeature;
use crate::searcher::planner_errors::map_regex_plan_error;
use crate::text_docs::authority_member_set;
use crate::{GenKey, TantivySearcher, normalize};
use quanta_index_contract::LqOptions;
use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_positions::query_phrase;
use quanta_index_lq_regex::RegexExecutor;
use quanta_index_lq_trigram::{
    DocId as TrigramDocId, LimitDimension, MAX_CANDIDATE_PRE_VERIFY, TrigramError,
    TrigramErrorCode, query_raw_substring, regex_prefilter_any_of,
};
use roaring::RoaringBitmap;
use std::sync::Arc;

use crate::searcher::preview_types::{
    PreviewResult, PreviewStop, SelectedSnippetSource, SnippetContext, integrity,
    token_allocation_bound,
};
use core::ops::Range;
use quanta_index_contract::{LqExpr, LqLeaf, LqPatternType, PreviewUnavailableReason};
use quanta_index_lq_regex::executor::RegexRangeError;
use quanta_index_lq_text_normalizer::MappedText;
use std::cell::Cell;

/// Verify-only regexes have the same pre-verify candidate cap as a usable
/// trigram prefilter.
///
/// Observe cancellation before advancing the authority
/// iterator and refuse an oversized set before allocating its next slot.
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
    ) -> Result<RoaringBitmap, CoreError> {
        let authority = self.text_authority(TextAuthorityFeature::RawSubstring)?;
        let folded = !Self::is_case_sensitive(options);
        let trigram_index = authority.trigram_index(folded);
        let resolver = authority.resolver(folded);
        let needle = normalize::nfc(needle);
        let query_bytes = normalize::apply_case(needle.as_ref(), Self::case_mode(options))
            .into_owned()
            .into_bytes();
        let verified_doc_ids = query_raw_substring(&trigram_index, &query_bytes, &resolver)
            .map_err(|err| match err.code {
                TrigramErrorCode::RegexPrefilterUnusable => CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::LexRawSubstringTrigramIndexMissing,
                    message: format!("lexical: raw substring requires verify-only fallback: {err}"),
                },
                TrigramErrorCode::PlanLimitExceeded
                | TrigramErrorCode::InvalidGeneration
                | TrigramErrorCode::IndexDeserialize
                | TrigramErrorCode::IndexCorrupted => {
                    map_trigram_error("raw substring prefilter", &err)
                }
            })?;
        authority_member_set(
            verified_doc_ids.iter().map(|doc_id| doc_id.0),
            "raw substring",
        )
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

    pub(crate) fn regex_timeout_budget_ms(options: &LqOptions) -> Option<u64> {
        options.timeout_ms
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
    ) -> Result<Arc<RoaringBitmap>, CoreError> {
        let normalized_source = Self::regex_source_for_options(source, options);
        let cache_key = options
            .timeout_ms
            .is_none()
            .then(|| self.regex_match_cache_key(&normalized_source));
        if let Some(cache_key) = cache_key.as_ref() {
            let mut cache = self.regex_match_cache.lock().map_err(|err| {
                CoreError::Storage(format!("lexical regex cache poisoned: {err}"))
            })?;
            // A hit shares the set; nothing is cloned per candidate.
            if let Some(cached) = cache.get(cache_key) {
                budget.checkpoint("lexical:regex-cache-hit")?;
                return Ok(cached);
            }
        }
        let authority = self.text_authority(TextAuthorityFeature::RegexTrigram)?;
        let plan = crate::regex::plan_regex(&normalized_source, options, &self.regex_policy)
            .map_err(map_regex_plan_error)?;
        let executor = RegexExecutor::compile(&normalized_source).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: regex plan/verify mismatch for {source:?}: {err}"
            ))
        })?;
        let folded = !Self::is_case_sensitive(options);
        let trigram_index = authority.trigram_index(folded);
        // The planner hands back an alternation, not a conjunction: a match
        // needs one of these literals. Case-insensitive patterns make that
        // concrete — `(?i)fresh` extracts `fresh` and `freſh` — so the prefilter
        // unions per alternative. AND-ing them filtered every document away.
        // Under `case:no` the folded trigram copy is searched, so every
        // alternative is folded with the same per-char fold that built it;
        // an alternative is a char-boundary prefix of some match, so its
        // fold is a prefix of the match's fold and the prefilter stays sound.
        let literal_alternation = if folded {
            plan.literal_alternation()
                .iter()
                .map(|literal| fold_literal_prefix(literal))
                .collect::<Vec<_>>()
        } else {
            plan.literal_alternation().to_vec()
        };
        let prefiltered_doc_ids = match regex_prefilter_any_of(&trigram_index, &literal_alternation)
        {
            Ok(doc_ids) => doc_ids,
            Err(err) if err.code == TrigramErrorCode::RegexPrefilterUnusable => {
                bounded_verify_only_candidates(authority.doc_ids(), budget)?
            }
            Err(err) => return Err(map_trigram_error("regex prefilter", &err)),
        };
        let resolver = authority.resolver(false);
        let budget_ms = Self::regex_timeout_budget_ms(options).unwrap_or(0);
        if options.timeout_ms == Some(0) && !prefiltered_doc_ids.is_empty() {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::QueryTimeout.into(),
                message:
                    "lexical: regex verify timed out before candidate verification began (budget 0ms)"
                        .to_string(),
            });
        }
        // The request budget is observed between candidates (W5 phase 2);
        // the probe looks every interval so a large candidate set costs
        // nothing extra per document.
        let probe = BudgetProbe::new(budget);
        let verified_doc_ids = executor
            .execute_interruptible(&prefiltered_doc_ids, &resolver, budget_ms, &|| {
                probe.tick()
            })
            .map_err(|err| match err.code {
                quanta_index_lq_regex::RegexErrorCode::QueryTimeout => CoreError::Typed {
                    code: LexicalErrorCode::QueryTimeout.into(),
                    message: format!("lexical: regex verify timed out: {err}"),
                },
                quanta_index_lq_regex::RegexErrorCode::Interrupted => probe
                    .interruption_error("lexical:regex-verify")
                    .unwrap_or_else(|| {
                        CoreError::Storage(format!(
                            "lexical: regex verify reported an interruption the budget probe did not observe: {err}"
                        ))
                    }),
                quanta_index_lq_regex::RegexErrorCode::ParseFail
                | quanta_index_lq_regex::RegexErrorCode::ForbiddenSyntax
                | quanta_index_lq_regex::RegexErrorCode::PlanLimitExceeded
                | quanta_index_lq_regex::RegexErrorCode::RegexPrefilterUnusable
                | quanta_index_lq_regex::RegexErrorCode::ExecutionInternal => CoreError::Typed {
                    code: crate::query_errors::regex_wire_code(err.code),
                    message: format!("lexical: regex verify failed: {err}"),
                },
            })?;
        // The interval probe can miss cancellation during its final few
        // candidates. Observe the request before materializing or publishing
        // the complete match set.
        budget.checkpoint("lexical:regex-verify-complete")?;
        let out = Arc::new(authority_member_set(
            verified_doc_ids.iter().map(|doc_id| doc_id.0),
            "regex",
        )?);
        let mut cache = self
            .regex_match_cache
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical regex cache poisoned: {err}")))?;
        cache.record_built(&out);
        if let Some(cache_key) = cache_key {
            // A result too wide for one entry is served but not kept; the
            // refusal is counted in the stats rather than logged.
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
    ) -> Result<RoaringBitmap, CoreError> {
        let authority = self.text_authority(TextAuthorityFeature::PhrasePositions)?;
        let plan = plan_phrase(
            text,
            options,
            &PhrasePolicy::defaults(),
            PhraseField::Content,
        )
        .map_err(map_phrase_plan_error)?;
        let positions_index = authority.positions_index(plan.case_sensitive);
        let terms = plan.tokens.iter().map(String::as_str).collect::<Vec<_>>();
        let matches = query_phrase(&positions_index, &terms)
            .map_err(|err| map_positions_error("phrase query", &err))?;
        if let Some(unheld) = matches
            .matches
            .iter()
            .find(|phrase_match| authority.doc(phrase_match.doc_id.0).is_none())
        {
            return Err(CoreError::Storage(format!(
                "lexical: phrase resolver missing doc {}",
                unheld.doc_id.0
            )));
        }
        authority_member_set(
            matches
                .matches
                .iter()
                .map(|phrase_match| phrase_match.doc_id.0),
            "phrase",
        )
    }
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
                    RegexRangeError::SourceByteLimit => {
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
