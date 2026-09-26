//! The unindexed scan: evaluating a query against stored documents directly.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::budgeted_search::{BudgetProbe, budgeted_search};
use crate::channel_payloads::count_from_len;
use crate::documents::stored_text;
use crate::documents::{file_name_for_path, language_from_path_hint};
use crate::metadata_normalize::normalize_language;
use crate::normalize::CaseMode;
use crate::predicate_registry::{
    ContentPathScope, ContentPredicateConstraint, ContributorPattern, FileContributorArg,
    FileOwnerArg, PREDICATE_OWNER, PredicateKind, kind_of, unimplemented_predicate,
};
use crate::query_errors::text_query_tokens;
use crate::ranked_page::{RankedRowView, group_in_memory, rank_in_memory};
use crate::searcher::candidates::SelectedPreviewContext;
use crate::{
    ManualPage, PreparedExecutableQuery, PreparedPredicatePlan, TantivySearcher, normalize,
};
use quanta_index_contract::{
    LexicalCandidate, LexicalRowOrderKey, LqExpr, LqFileScope, LqFilter, LqLeaf, LqOptions,
    LqPatternType, LqPredicateArg, LqQuery, QueryConstraintSetV1, SymbolCandidate,
};
use quanta_index_core::{
    CoreError, LexicalSearchPageV1, RequestBudgetV1, timeref::is_rev_at_time_spec,
};
use quanta_index_lq_regex::RegexExecutor;
use std::collections::BTreeSet;
use tantivy::DocSet as _;
use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, EnableScoring, Occur, Query, TermQuery};
use tantivy::schema::{IndexRecordOption, TantivyDocument};
use tantivy::{DocAddress, Term};

/// Identity rows keep the exact sealed document address through grouping and
/// ranking. Rendering never re-resolves an ID that another source may reuse.
pub(crate) struct ManualRankedCandidate<T> {
    candidate: T,
    address: DocAddress,
}

impl<T: RankedRowView> RankedRowView for ManualRankedCandidate<T> {
    fn ranked_key(&self) -> LexicalRowOrderKey<'_> {
        self.candidate.ranked_key()
    }
}

/// Borrowed fields from the same stored document throughout Boolean evaluation.
struct ManualDocumentView<'a> {
    doc: &'a TantivyDocument,
    source_repo_id: &'a str,
    repo_relative_path: &'a str,
    content: &'a str,
}

impl TantivySearcher {
    /// Whether `haystack` holds the token sequence of `text`, on the
    /// `index:no` route.
    ///
    /// The same normalizer as the inverted index and the position sidecar:
    /// a keyword is its token sequence and a phrase is a contiguous run of
    /// it, so the scan answers exactly what the indexed route answers,
    /// including the typed refusal of token-less or over-long literals.
    pub(crate) fn manual_token_sequence_matches(
        text: &str,
        haystack: &str,
        case: CaseMode,
    ) -> Result<bool, CoreError> {
        let wanted = text_query_tokens(text, case)?;
        let present: Vec<normalize::Token> = normalize::tokenize(haystack, case)
            .indexable()
            .cloned()
            .collect();
        Ok(normalize::contains_phrase(&present, &wanted))
    }

    pub(crate) fn doc_content_text(&self, doc: &TantivyDocument) -> String {
        stored_text(doc, self.fields.chunk_text)
            .or_else(|| stored_text(doc, self.fields.snippet))
            .unwrap_or_default()
    }

    pub(crate) fn doc_language(
        &self,
        doc: &TantivyDocument,
        repo_relative_path: &str,
    ) -> Option<String> {
        stored_text(doc, self.fields.language).or_else(|| {
            language_from_path_hint(repo_relative_path).map(std::string::ToString::to_string)
        })
    }

    pub(crate) fn manual_filter_regex(
        &self,
        pattern: &str,
        filter_name: &str,
    ) -> Result<RegexExecutor, CoreError> {
        RegexExecutor::compile(pattern).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: {filter_name} regex filter compile: {err}"
            ))
        })
    }

    pub(crate) fn manual_regex_matches(
        &self,
        source: &str,
        options: &LqOptions,
        haystack: &str,
    ) -> Result<bool, CoreError> {
        let normalized_source = Self::regex_source_for_options(source, options);
        let executor =
            RegexExecutor::compile(&normalized_source).map_err(|err| CoreError::Typed {
                code: crate::query_errors::regex_wire_code(err.code),
                message: format!(
                    "lexical: regex {source:?} failed to compile on unindexed scan route: {err}"
                ),
            })?;
        Ok(executor.verify(haystack.as_bytes()))
    }

    pub(crate) fn manual_doc_restrictions_allow(
        &self,
        prepared: &PreparedPredicatePlan,
        candidate_id: &str,
        repo_id: &str,
        repo_relative_path: &str,
    ) -> bool {
        if prepared
            .allowed_candidate_ids
            .as_ref()
            .is_some_and(|ids| !ids.contains(candidate_id))
        {
            return false;
        }
        if prepared
            .allowed_repo_ids
            .as_ref()
            .is_some_and(|ids| !ids.contains(repo_id))
        {
            return false;
        }
        if prepared
            .allowed_paths
            .as_ref()
            .is_some_and(|paths| !paths.contains(repo_relative_path))
        {
            return false;
        }
        true
    }

    pub(crate) fn manual_repo_gate_matches(
        &self,
        repo_ids: &BTreeSet<String>,
        repo_id: &str,
    ) -> bool {
        repo_ids.contains(repo_id)
    }

    pub(crate) fn manual_file_owner_matches(
        &self,
        arg: &FileOwnerArg,
        source_repo_id: &str,
        repo_relative_path: &str,
    ) -> Result<bool, CoreError> {
        let authority = self.file_ownership_authority()?;
        let Some(owners) = authority
            .owners_by_repo_id
            .get(source_repo_id)
            .and_then(|by_path| by_path.get(repo_relative_path))
        else {
            return Ok(false);
        };
        Ok(arg
            .owner
            .as_ref()
            .map_or(!owners.is_empty(), |owner| owners.contains(owner)))
    }

    pub(crate) fn manual_file_contributor_matches(
        &self,
        arg: &FileContributorArg,
        source_repo_id: &str,
        repo_relative_path: &str,
    ) -> Result<bool, CoreError> {
        let authority = self.file_contributor_authority()?;
        let Some(contributors) = authority
            .contributors_by_repo_id
            .get(source_repo_id)
            .and_then(|by_path| by_path.get(repo_relative_path))
        else {
            return Ok(false);
        };
        match &arg.contributor {
            ContributorPattern::Exact(contributor) => Ok(contributors
                .iter()
                .any(|identity| identity.canonical == *contributor)),
            ContributorPattern::Regex(source) => {
                let executor = RegexExecutor::compile(source).map_err(|err| CoreError::Typed {
                    code: crate::query_errors::regex_wire_code(err.code),
                    message: format!(
                        "lexical: file.has.contributor regex {source:?} failed to compile: {err}"
                    ),
                })?;
                Ok(contributors.iter().any(|identity| {
                    identity
                        .name
                        .as_deref()
                        .is_some_and(|name| executor.verify(name.as_bytes()))
                        || identity
                            .email
                            .as_deref()
                            .is_some_and(|email| executor.verify(email.as_bytes()))
                }))
            }
        }
    }

    pub(crate) fn manual_file_filter_matches(
        &self,
        pattern: &str,
        scope: LqFileScope,
        repo_relative_path: &str,
    ) -> Result<bool, CoreError> {
        let executor = self.manual_filter_regex(pattern, "file")?;
        Ok(Self::file_filter_scope_matches(
            &executor,
            scope,
            repo_relative_path,
        ))
    }

    /// Shared path/name scope semantics for document and coverage admission.
    pub(crate) fn file_filter_scope_matches(
        executor: &RegexExecutor,
        scope: LqFileScope,
        repo_relative_path: &str,
    ) -> bool {
        let path_match = executor.verify(repo_relative_path.as_bytes());
        match scope {
            LqFileScope::PathOnly => path_match,
            LqFileScope::NameOnly => file_name_for_path(repo_relative_path)
                .is_some_and(|name| executor.verify(name.as_bytes())),
            LqFileScope::NameAndPath => {
                path_match
                    || file_name_for_path(repo_relative_path)
                        .is_some_and(|name| executor.verify(name.as_bytes()))
            }
        }
    }

    pub(crate) fn manual_content_predicate_matches(
        &self,
        constraint: &ContentPredicateConstraint,
        options: &LqOptions,
        source_repo_id: &str,
        repo_relative_path: &str,
        content: &str,
        budget: &RequestBudgetV1,
    ) -> Result<bool, CoreError> {
        if let Some(ContentPathScope { pattern, scope }) = constraint.path_scope.as_ref()
            && !self.manual_file_filter_matches(pattern, *scope, repo_relative_path)?
        {
            return Ok(false);
        }
        if let Some(language) = constraint.language.as_ref() {
            let Some(normalized) = normalize_language(language) else {
                return Err(CoreError::InvalidContract(
                    "lexical: scoped content predicate escaped with an empty lang value"
                        .to_string(),
                ));
            };
            if self
                .doc_language(&TantivyDocument::new(), repo_relative_path)
                .as_deref()
                != Some(normalized.as_str())
            {
                return Ok(false);
            }
        }
        let lowered = self.predicate_content_leaf_from_constraint(constraint);
        self.manual_leaf_matches(
            &lowered,
            options,
            source_repo_id,
            repo_relative_path,
            content,
            false,
            budget,
        )
    }

    pub(crate) fn manual_predicate_matches(
        &self,
        name: &str,
        args: &[LqPredicateArg],
        options: &LqOptions,
        source_repo_id: &str,
        repo_relative_path: &str,
        content: &str,
        budget: &RequestBudgetV1,
    ) -> Result<bool, CoreError> {
        let Some((canonical_name, canonical_args)) =
            self.canonicalize_predicate_call(name, args)?
        else {
            return Err(unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
            )));
        };
        match kind_of(&canonical_name) {
            Some(PredicateKind::RepoFileGate) => {
                let constraint = self.repo_has_file_constraint(&canonical_name, &canonical_args)?;
                Ok(self.manual_repo_gate_matches(
                    &self.collect_repo_ids_for_repo_has_file(&constraint, options, budget)?,
                    source_repo_id,
                ))
            }
            Some(PredicateKind::RepoContentGate) => {
                let leaf = self.repo_content_constraint(&canonical_name, &canonical_args)?;
                Ok(self.manual_repo_gate_matches(
                    &self.collect_repo_ids_for_repo_has_content(&leaf, options, budget)?,
                    source_repo_id,
                ))
            }
            Some(PredicateKind::RepoCommitRecencyGate) => {
                let timeref =
                    self.repo_commit_after_constraint(&canonical_name, &canonical_args)?;
                Ok(self.manual_repo_gate_matches(
                    &self.collect_repo_ids_for_repo_has_commit_after(&timeref)?,
                    source_repo_id,
                ))
            }
            Some(PredicateKind::RepoMetaGate) => {
                let arg = self.repo_meta_constraint(&canonical_name, &canonical_args)?;
                Ok(self.manual_repo_gate_matches(
                    &self.collect_repo_ids_for_repo_has_meta(&arg)?,
                    source_repo_id,
                ))
            }
            Some(PredicateKind::RepoTopicGate) => {
                let arg = self.repo_topic_constraint(&canonical_name, &canonical_args)?;
                Ok(self.manual_repo_gate_matches(
                    &self.collect_repo_ids_for_repo_has_topic(&arg)?,
                    source_repo_id,
                ))
            }
            Some(PredicateKind::RepoDescriptionGate) => {
                let arg = self.repo_description_constraint(&canonical_name, &canonical_args)?;
                Ok(self.manual_repo_gate_matches(
                    &self.collect_repo_ids_for_repo_has_description(&arg)?,
                    source_repo_id,
                ))
            }
            Some(PredicateKind::FileOwnerGate) => {
                let arg = self.file_owner_constraint(&canonical_name, &canonical_args)?;
                self.manual_file_owner_matches(&arg, source_repo_id, repo_relative_path)
            }
            Some(PredicateKind::FileContributorGate) => {
                let arg = self.file_contributor_constraint(&canonical_name, &canonical_args)?;
                self.manual_file_contributor_matches(&arg, source_repo_id, repo_relative_path)
            }
            Some(PredicateKind::ContentLeaf) => {
                let constraint =
                    self.content_predicate_constraint(&canonical_name, &canonical_args)?;
                self.manual_content_predicate_matches(
                    &constraint,
                    options,
                    source_repo_id,
                    repo_relative_path,
                    content,
                    budget,
                )
            }
            None => Err(unimplemented_predicate(format!(
                "lexical: predicate leaf `{canonical_name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
            ))),
        }
    }

    pub(crate) fn manual_leaf_matches(
        &self,
        leaf: &LqLeaf,
        options: &LqOptions,
        source_repo_id: &str,
        repo_relative_path: &str,
        content: &str,
        include_path_terms: bool,
        budget: &RequestBudgetV1,
    ) -> Result<bool, CoreError> {
        let case = Self::case_mode(options);
        match leaf {
            LqLeaf::Keyword(text) => {
                if options.pattern_type == LqPatternType::Regexp {
                    return self.manual_regex_matches(text, options, content);
                }
                Ok(Self::manual_token_sequence_matches(text, content, case)?
                    || (include_path_terms
                        && Self::manual_token_sequence_matches(text, repo_relative_path, case)?))
            }
            LqLeaf::Phrase(text) => Self::manual_token_sequence_matches(text, content, case),
            LqLeaf::RawString(text) => {
                if options.pattern_type == LqPatternType::Regexp {
                    return self.manual_regex_matches(text, options, content);
                }
                Ok(normalize::contains_substring(content, text, case))
            }
            LqLeaf::Regex(text) => self.manual_regex_matches(text, options, content),
            LqLeaf::StructuralBlock(_) => Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::StrProducerParseTreeUnavailable,
                ),
                message: "lexical: structural leaf cannot execute on the unindexed scan route"
                    .to_string(),
            }),
            LqLeaf::Predicate { name, args } => self.manual_predicate_matches(
                name,
                args,
                options,
                source_repo_id,
                repo_relative_path,
                content,
                budget,
            ),
        }
    }

    fn manual_expr_matches(
        &self,
        view: &ManualDocumentView<'_>,
        expr: &LqExpr,
        options: &LqOptions,
        include_path_terms: bool,
        budget: &RequestBudgetV1,
    ) -> Result<bool, CoreError> {
        match expr {
            LqExpr::Leaf(LqLeaf::Predicate { name, args })
                if quanta_index_core::LexicalPredicateV1::from_canonical_name(name)
                    .is_some_and(|predicate| {
                        matches!(
                            predicate,
                            quanta_index_core::LexicalPredicateV1::SymbolLocalNameExact
                                | quanta_index_core::LexicalPredicateV1::SymbolQualifiedNameExact
                        )
                    }) =>
            {
                crate::symbol::exact_symbol_name_matches(
                    &self.fields,
                    view.doc,
                    name,
                    args,
                    options,
                )?
                .ok_or_else(|| {
                    CoreError::Storage("lexical: exact symbol predicate lost its policy".into())
                })
            }
            LqExpr::Empty => Ok(true),
            LqExpr::Leaf(leaf) => self.manual_leaf_matches(
                leaf,
                options,
                view.source_repo_id,
                view.repo_relative_path,
                view.content,
                include_path_terms,
                budget,
            ),
            LqExpr::All(children) => {
                for child in children {
                    if !self.manual_expr_matches(view, child, options, false, budget)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            LqExpr::Any(children) => {
                for child in children {
                    if self.manual_expr_matches(view, child, options, false, budget)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            LqExpr::Not(inner) => {
                Ok(!self.manual_expr_matches(view, inner, options, false, budget)?)
            }
        }
    }

    pub(crate) fn manual_filter_matches(
        &self,
        filter: &LqFilter,
        options: &LqOptions,
        source_repo_id: &str,
        repo_relative_path: &str,
        content: &str,
        budget: &RequestBudgetV1,
    ) -> Result<bool, CoreError> {
        match filter {
            LqFilter::Repo { pattern, revs } => {
                if !revs.is_empty() {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::REV_UNAVAILABLE,
                        message: "lexical: repo filter revisions require a history producer"
                            .to_string(),
                    });
                }
                let executor = self.manual_filter_regex(pattern, "repo")?;
                Ok(executor.verify(source_repo_id.as_bytes()))
            }
            LqFilter::File { pattern, scope } => {
                self.manual_file_filter_matches(pattern, *scope, repo_relative_path)
            }
            LqFilter::Content { leaf } => self.manual_leaf_matches(
                leaf,
                options,
                source_repo_id,
                repo_relative_path,
                content,
                false,
                budget,
            ),
            LqFilter::Lang { id } => {
                let Some(language) = normalize_language(id.as_str()) else {
                    return Err(CoreError::InvalidContract(
                        "lexical: lang filter value cannot be empty".to_string(),
                    ));
                };
                let doc_language = self.doc_language(&TantivyDocument::new(), repo_relative_path);
                Ok(
                    language_from_path_hint(repo_relative_path).or(doc_language.as_deref())
                        == Some(language.as_str()),
                )
            }
            LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => Ok(true),
            LqFilter::Rev { spec } => Err(CoreError::Typed {
                code: crate::filters::codes::REV_UNAVAILABLE,
                message: if is_rev_at_time_spec(spec) {
                    "lexical: rev:at.time(...) requires revision-selection and pin rebinding before lexical execution".to_string()
                } else {
                    "lexical: rev filter requires history producer".to_string()
                },
            }),
            LqFilter::Author { .. } => Err(CoreError::Typed {
                code: crate::filters::codes::AUTHOR_UNAVAILABLE,
                message: "lexical: author filter is not executable on the current adapter set"
                    .to_string(),
            }),
            LqFilter::Committer { .. } => Err(CoreError::Typed {
                code: crate::filters::codes::COMMITTER_UNAVAILABLE,
                message: "lexical: committer filter is not executable on the current adapter set"
                    .to_string(),
            }),
            LqFilter::Message { .. } => Err(CoreError::Typed {
                code: crate::filters::codes::MESSAGE_UNAVAILABLE,
                message: "lexical: message filter is not executable on the current adapter set"
                    .to_string(),
            }),
            LqFilter::Dirty { .. } => Err(CoreError::Typed {
                code: crate::filters::codes::DIRTY_UNAVAILABLE,
                message: "lexical: dirty filter is not executable on the current adapter set"
                    .to_string(),
            }),
            LqFilter::Changed { .. }
            | LqFilter::Stale { .. }
            | LqFilter::Snapshot { .. }
            | LqFilter::MetaOwner { .. }
            | LqFilter::MetaService { .. }
            | LqFilter::MetaLayer { .. }
            | LqFilter::MetaSurface { .. }
            | LqFilter::Affected { .. }
            | LqFilter::InvalidatedBy { .. } => Err(CoreError::Typed {
                code: crate::filters::codes::RUNTIME_CATALOG_UNAVAILABLE,
                message:
                    "lexical: runtime catalog filters are not executable on the current adapter set"
                        .to_string(),
            }),
            LqFilter::Before { .. }
            | LqFilter::After { .. }
            | LqFilter::Since { .. }
            | LqFilter::Until { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. } => Err(CoreError::Typed {
                code: crate::filters::codes::HISTORY_PRODUCER_UNAVAILABLE,
                message: "lexical: history date/diff filters require history producer".to_string(),
            }),
            LqFilter::Type { .. } | LqFilter::Select { .. } => Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexFilterUnrouted,
                message: format!(
                    "lexical: type/select filters must be routed through doc-kind preparation, got `{filter:?}`"
                ),
            }),
        }
    }

    /// Explicit `index:no` execution: every document of the generation is
    /// fetched and matched in memory, so the corpus itself is the examined
    /// set and must fit the budget before the scan starts.
    pub(crate) fn manual_text_search(
        &self,
        query: &LqQuery,
        prepared: &PreparedExecutableQuery,
        constraints: &QueryConstraintSetV1,
        page: &ManualPage<'_>,
        budget: &RequestBudgetV1,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        let searcher = self.reader.searcher();
        let doc_limit = Self::corpus_docs(&searcher, "unindexed text scan")?;
        if doc_limit > self.execution_budget.max_examined_candidates() {
            return Err(self
                .execution_budget
                .exceeded("unindexed text scan (index:no)"));
        }
        if doc_limit == 0 {
            return Ok(LexicalSearchPageV1 {
                candidates: Vec::new(),
                exact_total: Some(0),
            });
        }
        let hits = budgeted_search(
            &searcher,
            &AllQuery,
            &TopDocs::with_limit(doc_limit),
            budget,
            "lexical:scan",
        )?;
        let boosted_score = Self::apply_query_boost_score(1.0, &query.options);
        let mut out: Vec<ManualRankedCandidate<LexicalCandidate>> = Vec::new();
        // The scan matches every document against the plan itself; the
        // budget is observed between documents (W5 phase 2).
        let probe = BudgetProbe::new(budget);
        for (_score, doc_address) in hits {
            if probe.tick()
                && let Some(interruption) = probe.interruption_error("lexical:scan")
            {
                return Err(interruption);
            }
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if !self.manual_doc_matches(&doc, query, prepared, constraints, budget)? {
                continue;
            }
            out.push(ManualRankedCandidate {
                candidate: self.document_to_candidate_identity(&doc, boosted_score)?,
                address: doc_address,
            });
        }
        let out = match page.group {
            Some(group) => group_in_memory(out, group),
            None => out,
        };
        let mut out = rank_in_memory(out, page.after);
        // The scan visited every document, so the total after the boundary
        // is exact for free.
        let exact_total = Some(count_from_len(out.len())?);
        out.truncate(page.limit);
        let mut preview = self.selected_preview_context(query, &prepared.predicate_plan, budget)?;
        Ok(LexicalSearchPageV1 {
            candidates: self.render_manual_candidates(
                out,
                &mut preview,
                Self::document_to_candidate,
            )?,
            exact_total,
        })
    }

    /// Only the retained page's rows cross the preview boundary. One caller
    /// context accounts for all rows, including retained output reservations.
    pub(crate) fn render_manual_candidates<T: RankedRowView>(
        &self,
        rows: Vec<ManualRankedCandidate<T>>,
        preview: &mut SelectedPreviewContext<'_>,
        convert: fn(
            &Self,
            &TantivyDocument,
            f32,
            &mut SelectedPreviewContext<'_>,
        ) -> Result<T, CoreError>,
    ) -> Result<Vec<T>, CoreError> {
        let searcher = self.reader.searcher();
        let candidates = rows
            .into_iter()
            .map(|row| {
                let doc: TantivyDocument = searcher.doc(row.address).map_err(|error| {
                    CoreError::Storage(format!(
                        "lexical: fetch selected manual doc {:?}: {error}",
                        row.address
                    ))
                })?;
                let candidate = convert(self, &doc, row.ranked_key().score, preview)?;
                if candidate.ranked_key().order(&row.ranked_key()) != std::cmp::Ordering::Equal {
                    return Err(CoreError::Storage(format!(
                        "lexical: selected manual doc {:?} changed its ranked identity",
                        row.address
                    )));
                }
                Ok(candidate)
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        preview.retain_output_for_request()?;
        Ok(candidates)
    }

    /// Whether one stored document matches the unindexed-scan plan.
    ///
    /// The scan and the per-candidate explanation (QI-BB-022) share this so
    /// a candidate explains through exactly the matcher that ranked it.
    pub(crate) fn manual_doc_matches(
        &self,
        doc: &TantivyDocument,
        query: &LqQuery,
        prepared: &PreparedExecutableQuery,
        constraints: &QueryConstraintSetV1,
        budget: &RequestBudgetV1,
    ) -> Result<bool, CoreError> {
        if stored_text(doc, self.fields.doc_kind).as_deref() != Some(prepared.doc_kind.as_str()) {
            return Ok(false);
        }
        let Some(candidate_id) = stored_text(doc, self.fields.candidate_id) else {
            return Ok(false);
        };
        let Some(source_repo_id) = stored_text(doc, self.fields.repo_id) else {
            return Ok(false);
        };
        let Some(repo_relative_path) = stored_text(doc, self.fields.repo_relative_path) else {
            return Ok(false);
        };
        if !Self::manual_exact_path_allows(&repo_relative_path, constraints) {
            return Ok(false);
        }
        if !self.manual_doc_restrictions_allow(
            &prepared.predicate_plan,
            &candidate_id,
            &source_repo_id,
            &repo_relative_path,
        ) {
            return Ok(false);
        }
        let include_path_terms =
            Self::enables_path_term_surface(&prepared.predicate_plan.expr, &query.options);
        let content = self.doc_content_text(doc);
        let view = ManualDocumentView {
            doc,
            source_repo_id: &source_repo_id,
            repo_relative_path: &repo_relative_path,
            content: &content,
        };
        if !self.manual_expr_matches(
            &view,
            &prepared.predicate_plan.expr,
            &query.options,
            include_path_terms,
            budget,
        )? {
            return Ok(false);
        }
        for filter in &prepared.query.filters {
            if !self.manual_filter_matches(
                filter,
                &query.options,
                &source_repo_id,
                &repo_relative_path,
                &content,
                budget,
            )? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The one live document with `candidate_id` of `doc_kind`, by exact
    /// term lookup (QI-BB-022).
    ///
    /// Two live documents under one id would mean the writer's
    /// delete-then-add upsert did not hold; that is a corrupt index, not a
    /// choice to make here.
    pub(crate) fn locate_candidate(
        &self,
        searcher: &tantivy::Searcher,
        candidate_id: &str,
        doc_kind: &str,
    ) -> Result<Option<(DocAddress, TantivyDocument)>, CoreError> {
        let id_clause: Box<dyn Query> = Box::new(TermQuery::new(
            Term::from_field_text(self.fields.candidate_id, candidate_id),
            IndexRecordOption::Basic,
        ));
        let kind_clause: Box<dyn Query> = Box::new(TermQuery::new(
            Term::from_field_text(self.fields.doc_kind, doc_kind),
            IndexRecordOption::Basic,
        ));
        let lookup = BooleanQuery::new(vec![(Occur::Must, id_clause), (Occur::Must, kind_clause)]);
        let hits = searcher
            .search(&lookup, &TopDocs::with_limit(2))
            .map_err(|err| CoreError::Storage(format!("lexical: candidate lookup: {err}")))?;
        if hits.len() > 1 {
            return Err(CoreError::Storage(format!(
                "lexical: candidate id `{candidate_id}` names {} live documents",
                hits.len()
            )));
        }
        let Some((_score, doc_address)) = hits.into_iter().next() else {
            return Ok(None);
        };
        let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
            CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
        })?;
        Ok(Some((doc_address, doc)))
    }

    /// Score one live document through the compiled plan, exactly as the
    /// ranked collector would: the plan's scorer positioned on the document.
    pub(crate) fn score_one_document(
        searcher: &tantivy::Searcher,
        compiled: &dyn Query,
        doc_address: DocAddress,
    ) -> Result<Option<f32>, CoreError> {
        let weight = compiled
            .weight(EnableScoring::enabled_from_searcher(searcher))
            .map_err(|err| CoreError::Storage(format!("lexical: explain weight: {err}")))?;
        let reader = searcher.segment_reader(doc_address.segment_ord);
        let mut scorer = weight
            .scorer(reader, 1.0)
            .map_err(|err| CoreError::Storage(format!("lexical: explain scorer: {err}")))?;
        // A fresh scorer sits on its first matching document; `seek` only
        // moves forward, so a document before that first match is simply not
        // matched.
        let target = doc_address.doc_id;
        let first = scorer.doc();
        let landed = if first >= target {
            first
        } else {
            scorer.seek(target)
        };
        if landed != target {
            return Ok(None);
        }
        Ok(Some(scorer.score()))
    }

    /// Every symbol document the `index:no` plan matches, unordered.
    pub(crate) fn manual_symbol_matches(
        &self,
        query: &LqQuery,
        prepared: &PreparedExecutableQuery,
        constraints: &QueryConstraintSetV1,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<ManualRankedCandidate<SymbolCandidate>>, CoreError> {
        let searcher = self.reader.searcher();
        let doc_limit = Self::corpus_docs(&searcher, "unindexed symbol scan")?;
        if doc_limit > self.execution_budget.max_examined_candidates() {
            return Err(self
                .execution_budget
                .exceeded("unindexed symbol scan (index:no)"));
        }
        if doc_limit == 0 {
            return Ok(Vec::new());
        }
        let hits = budgeted_search(
            &searcher,
            &AllQuery,
            &TopDocs::with_limit(doc_limit),
            budget,
            "lexical:scan",
        )?;
        let boosted_score = Self::apply_query_boost_score(1.0, &query.options);
        let mut out: Vec<ManualRankedCandidate<SymbolCandidate>> = Vec::new();
        let probe = BudgetProbe::new(budget);
        for (_score, doc_address) in hits {
            if probe.tick()
                && let Some(interruption) = probe.interruption_error("lexical:scan")
            {
                return Err(interruption);
            }
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if !self.manual_doc_matches(&doc, query, prepared, constraints, budget)? {
                continue;
            }
            out.push(ManualRankedCandidate {
                candidate: self.document_to_symbol_candidate_identity(&doc, boosted_score)?,
                address: doc_address,
            });
        }
        Ok(out)
    }

    pub(crate) fn missing_repo_metadata_error(filter_name: &str) -> CoreError {
        CoreError::NotReady(format!(
            "lexical: repo metadata snapshot missing for filter `{filter_name}` in LexicalFullBundle.payload"
        ))
    }
}
