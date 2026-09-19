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
use crate::searcher::planner_errors::map_regex_plan_error;
use crate::text_docs::authority_member_set;
use crate::{GenKey, TantivySearcher, normalize};
use quanta_index_contract::LqOptions;
use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_positions::query_phrase;
use quanta_index_lq_regex::RegexExecutor;
use quanta_index_lq_trigram::{
    DocId as TrigramDocId, TrigramErrorCode, query_raw_substring, regex_prefilter_any_of,
};
use roaring::RoaringBitmap;
use std::sync::Arc;

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
        let authority = self.text_authority("LEX_RAW_SUBSTRING")?;
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
                    code: "LEX_RAW_SUBSTRING_TRIGRAM_INDEX_MISSING".to_string(),
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
                return Ok(cached);
            }
        }
        let authority = self.text_authority("LEX_REGEX_TRIGRAM")?;
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
                authority.doc_ids().map(TrigramDocId).collect()
            }
            Err(err) => return Err(map_trigram_error("regex prefilter", &err)),
        };
        let resolver = authority.resolver(false);
        let budget_ms = Self::regex_timeout_budget_ms(options).unwrap_or(0);
        if options.timeout_ms == Some(0) && !prefiltered_doc_ids.is_empty() {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::QueryTimeout.as_code_str().to_string(),
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
                    code: LexicalErrorCode::QueryTimeout.as_code_str().to_string(),
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
                    code: format!("LEX_REGEX_{}", err.code.as_code_str()),
                    message: format!("lexical: regex verify failed: {err}"),
                },
            })?;
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
        let authority = self.text_authority("LEX_PHRASE_POSITIONS")?;
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
