//! The unindexed scan: evaluating a query against stored documents directly.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::channel_payloads::count_from_len;
use crate::documents::{file_name_for_path, required_stored_text, stored_doc_kind};
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
use std::collections::{BTreeMap, BTreeSet};
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

/// Reusable predicate results live for one unindexed scan or candidate-admission pass.
///
/// Regex keys include the exact execution pattern and query option flags.
/// Repo-gate keys include the canonical predicate, ordered arguments, and
/// options; the gate set is collected at most once for the request. A fixed
/// regex entry cap prevents query complexity from growing retained engines
/// without bound; a regex miss evicts one entry before compilation.
const MANUAL_REGEX_CACHE_ENTRIES: usize = 4;

struct ManualRepoGate {
    name: String,
    args: Vec<LqPredicateArg>,
    options: LqOptions,
    repo_ids: BTreeSet<String>,
}

#[derive(Default)]
pub(crate) struct ManualScanCache {
    compiled: BTreeMap<String, RegexExecutor>,
    repo_gates: Vec<ManualRepoGate>,
    #[cfg(test)]
    compiled_builds: usize,
}

impl ManualScanCache {
    fn repo_gate_ids(
        &mut self,
        name: &str,
        args: &[LqPredicateArg],
        options: &LqOptions,
        collect: impl FnOnce() -> Result<BTreeSet<String>, CoreError>,
    ) -> Result<&BTreeSet<String>, CoreError> {
        if let Some(index) = self.repo_gates.iter().position(|cached| {
            cached.name == name && cached.args == args && cached.options == *options
        }) {
            return self
                .repo_gates
                .get(index)
                .map(|gate| &gate.repo_ids)
                .ok_or_else(|| {
                    CoreError::Storage("lexical: manual repo gate cache lost a known entry".into())
                });
        }
        let ids = collect()?;
        self.repo_gates.push(ManualRepoGate {
            name: name.to_owned(),
            args: args.to_vec(),
            options: options.clone(),
            repo_ids: ids,
        });
        let last = self.repo_gates.last().ok_or_else(|| {
            CoreError::Storage("lexical: manual repo gate cache lost its inserted entry".into())
        })?;
        Ok(&last.repo_ids)
    }

    fn get_or_compile(
        &mut self,
        pattern: &str,
        compile: impl FnOnce() -> Result<RegexExecutor, CoreError>,
    ) -> Result<&RegexExecutor, CoreError> {
        use std::collections::btree_map::Entry;
        if self.compiled.contains_key(pattern) {
            return self.compiled.get(pattern).ok_or_else(|| {
                CoreError::Storage("lexical: manual regex cache lost a known entry".into())
            });
        }
        if self.compiled.len() == MANUAL_REGEX_CACHE_ENTRIES {
            let _evicted = self.compiled.pop_first();
        }
        match self.compiled.entry(pattern.to_owned()) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => {
                let compiled = compile()?;
                #[cfg(test)]
                {
                    self.compiled_builds = self.compiled_builds.saturating_add(1);
                }
                Ok(entry.insert(compiled))
            }
        }
    }

    fn regex_matches(
        &mut self,
        source: &str,
        options: &LqOptions,
        haystack: &str,
    ) -> Result<bool, CoreError> {
        let normalized_source = TantivySearcher::regex_source_for_options(source, options);
        let executor = self.get_or_compile(&normalized_source, || {
            RegexExecutor::compile(&normalized_source).map_err(|err| CoreError::Typed {
                code: crate::query_errors::regex_wire_code(err.code),
                message: format!(
                    "lexical: regex {source:?} failed to compile on unindexed scan route: {err}"
                ),
            })
        })?;
        Ok(executor.verify(haystack.as_bytes()))
    }
}

fn manual_language_unavailable() -> CoreError {
    CoreError::NotImplemented(
        "lexical: index:no language filtering requires indexed execution; language is not stored"
            .to_string(),
    )
}

impl TantivySearcher {
    fn ensure_manual_language_leaf_supported(&self, leaf: &LqLeaf) -> Result<(), CoreError> {
        let LqLeaf::Predicate { name, args } = leaf else {
            return Ok(());
        };
        let Some((canonical_name, canonical_args)) =
            self.canonicalize_predicate_call(name, args)?
        else {
            return Ok(());
        };
        if kind_of(&canonical_name) == Some(PredicateKind::ContentLeaf)
            && self
                .content_predicate_constraint(&canonical_name, &canonical_args)?
                .language
                .is_some()
        {
            return Err(manual_language_unavailable());
        }
        Ok(())
    }

    fn ensure_manual_language_expr_supported(&self, expr: &LqExpr) -> Result<(), CoreError> {
        match expr {
            LqExpr::Leaf(leaf) => self.ensure_manual_language_leaf_supported(leaf),
            LqExpr::All(children) | LqExpr::Any(children) => {
                for child in children {
                    self.ensure_manual_language_expr_supported(child)?;
                }
                Ok(())
            }
            LqExpr::Not(inner) => self.ensure_manual_language_expr_supported(inner),
            LqExpr::Empty => Ok(()),
        }
    }

    pub(crate) fn ensure_manual_language_query_supported(
        &self,
        query: &LqQuery,
    ) -> Result<(), CoreError> {
        self.ensure_manual_language_expr_supported(&query.expr)?;
        for filter in &query.filters {
            match filter {
                LqFilter::Lang { id } => {
                    if normalize_language(id).is_none() {
                        return Err(CoreError::InvalidContract(
                            "lexical: lang filter value cannot be empty".to_string(),
                        ));
                    }
                    return Err(manual_language_unavailable());
                }
                LqFilter::Content { leaf } => self.ensure_manual_language_leaf_supported(leaf)?,
                LqFilter::Repo { .. }
                | LqFilter::File { .. }
                | LqFilter::Rev { .. }
                | LqFilter::Author { .. }
                | LqFilter::Committer { .. }
                | LqFilter::Message { .. }
                | LqFilter::Before { .. }
                | LqFilter::After { .. }
                | LqFilter::Since { .. }
                | LqFilter::Until { .. }
                | LqFilter::DiffAdded { .. }
                | LqFilter::DiffRemoved { .. }
                | LqFilter::DiffTouched { .. }
                | LqFilter::Type { .. }
                | LqFilter::Select { .. }
                | LqFilter::Dirty { .. }
                | LqFilter::Changed { .. }
                | LqFilter::Stale { .. }
                | LqFilter::Snapshot { .. }
                | LqFilter::MetaOwner { .. }
                | LqFilter::MetaService { .. }
                | LqFilter::MetaLayer { .. }
                | LqFilter::MetaSurface { .. }
                | LqFilter::Affected { .. }
                | LqFilter::InvalidatedBy { .. }
                | LqFilter::Fork { .. }
                | LqFilter::Archived { .. }
                | LqFilter::Visibility { .. }
                | LqFilter::Context { .. } => {}
            }
        }
        Ok(())
    }

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

    pub(crate) fn doc_content_text<'a>(
        &self,
        doc: &'a TantivyDocument,
    ) -> Result<&'a str, CoreError> {
        required_stored_text(doc, self.fields.chunk_text, "chunk_text")
    }

    pub(crate) fn manual_filter_regex(
        pattern: &str,
        filter_name: &str,
    ) -> Result<RegexExecutor, CoreError> {
        RegexExecutor::compile(pattern).map_err(|err| {
            let message = format!("lexical: {filter_name} regex filter compile: {err}");
            if err.code == quanta_index_lq_regex::RegexErrorCode::PlanLimitExceeded {
                CoreError::Typed {
                    code: crate::query_errors::regex_wire_code(err.code),
                    message,
                }
            } else {
                CoreError::InvalidContract(message)
            }
        })
    }

    pub(crate) fn manual_regex_matches(
        &self,
        source: &str,
        options: &LqOptions,
        haystack: &str,
        regex_cache: &mut ManualScanCache,
    ) -> Result<bool, CoreError> {
        regex_cache.regex_matches(source, options, haystack)
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
        regex_cache: &mut ManualScanCache,
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
                let executor = regex_cache.get_or_compile(source, || {
                    RegexExecutor::compile(source).map_err(|err| CoreError::Typed {
                        code: crate::query_errors::regex_wire_code(err.code),
                        message: format!(
                            "lexical: file.has.contributor regex {source:?} failed to compile: {err}"
                        ),
                    })
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
        regex_cache: &mut ManualScanCache,
    ) -> Result<bool, CoreError> {
        let executor =
            regex_cache.get_or_compile(pattern, || Self::manual_filter_regex(pattern, "file"))?;
        Ok(Self::file_filter_scope_matches(
            executor,
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

    #[expect(
        clippy::too_many_arguments,
        reason = "the private content predicate evaluator carries source identity and request-local regex state"
    )]
    pub(crate) fn manual_content_predicate_matches(
        &self,
        doc: &TantivyDocument,
        constraint: &ContentPredicateConstraint,
        options: &LqOptions,
        source_repo_id: &str,
        repo_relative_path: &str,
        content: &str,
        budget: &RequestBudgetV1,
        regex_cache: &mut ManualScanCache,
    ) -> Result<bool, CoreError> {
        if let Some(ContentPathScope { pattern, scope }) = constraint.path_scope.as_ref()
            && !self.manual_file_filter_matches(pattern, *scope, repo_relative_path, regex_cache)?
        {
            return Ok(false);
        }
        if constraint.language.is_some() {
            return Err(manual_language_unavailable());
        }
        let lowered = self.predicate_content_leaf_from_constraint(constraint);
        self.manual_leaf_matches(
            doc,
            &lowered,
            options,
            source_repo_id,
            repo_relative_path,
            content,
            false,
            budget,
            regex_cache,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the private manual predicate route needs the immutable document, source identity, query options and request budget together"
    )]
    pub(crate) fn manual_predicate_matches(
        &self,
        doc: &TantivyDocument,
        name: &str,
        args: &[LqPredicateArg],
        options: &LqOptions,
        source_repo_id: &str,
        repo_relative_path: &str,
        content: &str,
        budget: &RequestBudgetV1,
        regex_cache: &mut ManualScanCache,
    ) -> Result<bool, CoreError> {
        let Some((canonical_name, canonical_args)) =
            self.canonicalize_predicate_call(name, args)?
        else {
            return Err(unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
            )));
        };
        match kind_of(&canonical_name) {
            Some(PredicateKind::RepoFileGate) => Ok(self.manual_repo_gate_matches(
                regex_cache.repo_gate_ids(&canonical_name, &canonical_args, options, || {
                    let constraint =
                        self.repo_has_file_constraint(&canonical_name, &canonical_args)?;
                    self.collect_repo_ids_for_repo_has_file(&constraint, options, budget)
                })?,
                source_repo_id,
            )),
            Some(PredicateKind::RepoContentGate) => Ok(self.manual_repo_gate_matches(
                regex_cache.repo_gate_ids(&canonical_name, &canonical_args, options, || {
                    let leaf = self.repo_content_constraint(&canonical_name, &canonical_args)?;
                    self.collect_repo_ids_for_repo_has_content(&leaf, options, budget)
                })?,
                source_repo_id,
            )),
            Some(PredicateKind::RepoCommitRecencyGate) => Ok(self.manual_repo_gate_matches(
                regex_cache.repo_gate_ids(&canonical_name, &canonical_args, options, || {
                    let timeref =
                        self.repo_commit_after_constraint(&canonical_name, &canonical_args)?;
                    self.collect_repo_ids_for_repo_has_commit_after(&timeref)
                })?,
                source_repo_id,
            )),
            Some(PredicateKind::RepoMetaGate) => Ok(self.manual_repo_gate_matches(
                regex_cache.repo_gate_ids(&canonical_name, &canonical_args, options, || {
                    let arg = self.repo_meta_constraint(&canonical_name, &canonical_args)?;
                    self.collect_repo_ids_for_repo_has_meta(&arg)
                })?,
                source_repo_id,
            )),
            Some(PredicateKind::RepoTopicGate) => Ok(self.manual_repo_gate_matches(
                regex_cache.repo_gate_ids(&canonical_name, &canonical_args, options, || {
                    let arg = self.repo_topic_constraint(&canonical_name, &canonical_args)?;
                    self.collect_repo_ids_for_repo_has_topic(&arg)
                })?,
                source_repo_id,
            )),
            Some(PredicateKind::RepoDescriptionGate) => Ok(self.manual_repo_gate_matches(
                regex_cache.repo_gate_ids(&canonical_name, &canonical_args, options, || {
                    let arg = self.repo_description_constraint(&canonical_name, &canonical_args)?;
                    self.collect_repo_ids_for_repo_has_description(&arg)
                })?,
                source_repo_id,
            )),
            Some(PredicateKind::FileOwnerGate) => {
                let arg = self.file_owner_constraint(&canonical_name, &canonical_args)?;
                self.manual_file_owner_matches(&arg, source_repo_id, repo_relative_path)
            }
            Some(PredicateKind::FileContributorGate) => {
                let arg = self.file_contributor_constraint(&canonical_name, &canonical_args)?;
                self.manual_file_contributor_matches(
                    &arg,
                    source_repo_id,
                    repo_relative_path,
                    regex_cache,
                )
            }
            Some(PredicateKind::ContentLeaf) => {
                let constraint =
                    self.content_predicate_constraint(&canonical_name, &canonical_args)?;
                self.manual_content_predicate_matches(
                    doc,
                    &constraint,
                    options,
                    source_repo_id,
                    repo_relative_path,
                    content,
                    budget,
                    regex_cache,
                )
            }
            None => Err(unimplemented_predicate(format!(
                "lexical: predicate leaf `{canonical_name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
            ))),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the private leaf evaluator shares the document, source identity, path policy and request budget with its predicate fallback"
    )]
    pub(crate) fn manual_leaf_matches(
        &self,
        doc: &TantivyDocument,
        leaf: &LqLeaf,
        options: &LqOptions,
        source_repo_id: &str,
        repo_relative_path: &str,
        content: &str,
        include_path_terms: bool,
        budget: &RequestBudgetV1,
        regex_cache: &mut ManualScanCache,
    ) -> Result<bool, CoreError> {
        let case = Self::case_mode(options);
        match leaf {
            LqLeaf::Keyword(text) => {
                if options.pattern_type == LqPatternType::Regexp {
                    return self.manual_regex_matches(text, options, content, regex_cache);
                }
                Ok(Self::manual_token_sequence_matches(text, content, case)?
                    || (include_path_terms
                        && Self::manual_token_sequence_matches(text, repo_relative_path, case)?))
            }
            LqLeaf::Phrase(text) => Self::manual_token_sequence_matches(text, content, case),
            LqLeaf::RawString(text) => {
                if options.pattern_type == LqPatternType::Regexp {
                    return self.manual_regex_matches(text, options, content, regex_cache);
                }
                Ok(normalize::contains_substring(content, text, case))
            }
            LqLeaf::Regex(text) => self.manual_regex_matches(text, options, content, regex_cache),
            LqLeaf::StructuralBlock(_) => Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::StrProducerParseTreeUnavailable,
                ),
                message: "lexical: structural leaf cannot execute on the unindexed scan route"
                    .to_string(),
            }),
            LqLeaf::Predicate { name, args } => self.manual_predicate_matches(
                doc,
                name,
                args,
                options,
                source_repo_id,
                repo_relative_path,
                content,
                budget,
                regex_cache,
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
        regex_cache: &mut ManualScanCache,
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
                view.doc,
                leaf,
                options,
                view.source_repo_id,
                view.repo_relative_path,
                view.content,
                include_path_terms,
                budget,
                regex_cache,
            ),
            LqExpr::All(children) => {
                for child in children {
                    if !self.manual_expr_matches(
                        view,
                        child,
                        options,
                        false,
                        budget,
                        regex_cache,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            LqExpr::Any(children) => {
                for child in children {
                    if self.manual_expr_matches(view, child, options, false, budget, regex_cache)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            LqExpr::Not(inner) => {
                Ok(!self.manual_expr_matches(view, inner, options, false, budget, regex_cache)?)
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the private filter evaluator carries document identity and request-local regex state"
    )]
    pub(crate) fn manual_filter_matches(
        &self,
        doc: &TantivyDocument,
        filter: &LqFilter,
        options: &LqOptions,
        source_repo_id: &str,
        repo_relative_path: &str,
        content: &str,
        budget: &RequestBudgetV1,
        regex_cache: &mut ManualScanCache,
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
                let executor = regex_cache
                    .get_or_compile(pattern, || Self::manual_filter_regex(pattern, "repo"))?;
                Ok(executor.verify(source_repo_id.as_bytes()))
            }
            LqFilter::File { pattern, scope } => {
                self.manual_file_filter_matches(pattern, *scope, repo_relative_path, regex_cache)
            }
            LqFilter::Content { leaf } => self.manual_leaf_matches(
                doc,
                leaf,
                options,
                source_repo_id,
                repo_relative_path,
                content,
                false,
                budget,
                regex_cache,
            ),
            LqFilter::Lang { id } => {
                if normalize_language(id.as_str()).is_none() {
                    return Err(CoreError::InvalidContract(
                        "lexical: lang filter value cannot be empty".to_string(),
                    ));
                }
                Err(manual_language_unavailable())
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
        self.ensure_manual_language_query_supported(query)?;
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
        let hits = self.collect_whole_set(
            &searcher,
            &AllQuery,
            1.0,
            "unindexed text scan (index:no)",
            budget,
        )?;
        let boosted_score = Self::apply_query_boost_score(1.0, &query.options);
        let mut out: Vec<ManualRankedCandidate<LexicalCandidate>> = Vec::new();
        let mut regex_cache = ManualScanCache::default();
        // The scan matches every document against the plan itself; the
        // budget is observed between documents (W5 phase 2).
        for row in hits {
            budget.checkpoint("lexical:scan")?;
            let doc_address = row.address;
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if !self.manual_doc_matches(
                &doc,
                query,
                prepared,
                constraints,
                budget,
                &mut regex_cache,
            )? {
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
        regex_cache: &mut ManualScanCache,
    ) -> Result<bool, CoreError> {
        if stored_doc_kind(doc, self.fields.doc_kind)? != prepared.doc_kind.as_str() {
            return Ok(false);
        }
        let candidate_id = required_stored_text(doc, self.fields.candidate_id, "candidate_id")?;
        let source_repo_id = required_stored_text(doc, self.fields.repo_id, "repo_id")?;
        let repo_relative_path =
            required_stored_text(doc, self.fields.repo_relative_path, "repo_relative_path")?;
        if !Self::manual_exact_path_allows(repo_relative_path, constraints) {
            return Ok(false);
        }
        if !self.manual_doc_restrictions_allow(
            &prepared.predicate_plan,
            candidate_id,
            source_repo_id,
            repo_relative_path,
        ) {
            return Ok(false);
        }
        let include_path_terms =
            Self::enables_path_term_surface(&prepared.predicate_plan.expr, &query.options);
        let content = self.doc_content_text(doc)?;
        let view = ManualDocumentView {
            doc,
            source_repo_id,
            repo_relative_path,
            content,
        };
        if !self.manual_expr_matches(
            &view,
            &prepared.predicate_plan.expr,
            &query.options,
            include_path_terms,
            budget,
            regex_cache,
        )? {
            return Ok(false);
        }
        for filter in &prepared.query.filters {
            if !self.manual_filter_matches(
                doc,
                filter,
                &query.options,
                source_repo_id,
                repo_relative_path,
                content,
                budget,
                regex_cache,
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
        self.ensure_manual_language_query_supported(query)?;
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
        let hits = self.collect_whole_set(
            &searcher,
            &AllQuery,
            1.0,
            "unindexed symbol scan (index:no)",
            budget,
        )?;
        let boosted_score = Self::apply_query_boost_score(1.0, &query.options);
        let mut out: Vec<ManualRankedCandidate<SymbolCandidate>> = Vec::new();
        let mut regex_cache = ManualScanCache::default();
        for row in hits {
            budget.checkpoint("lexical:scan")?;
            let doc_address = row.address;
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if !self.manual_doc_matches(
                &doc,
                query,
                prepared,
                constraints,
                budget,
                &mut regex_cache,
            )? {
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

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "corrupt stored-field cases assert fail-closed scan behavior"
)]
mod stored_authority_tests {
    use super::*;
    use quanta_index_contract::LqCase;
    use std::cell::Cell;
    use tantivy::schema::{STORED, Schema};

    #[test]
    fn manual_matcher_reuses_compilation_across_documents_and_separates_case_modes()
    -> Result<(), CoreError> {
        let mut cache = ManualScanCache::default();
        let folded = LqOptions::defaults();
        for (content, expected) in [("needle42", true), ("absent", false), ("NEEDLE43", true)] {
            assert_eq!(
                cache.regex_matches("needle[0-9]+", &folded, content)?,
                expected
            );
        }
        assert_eq!(cache.compiled_builds, 1);
        let mut sensitive = folded;
        sensitive.case = Some(LqCase::Sensitive);
        assert!(!cache.regex_matches("needle[0-9]+", &sensitive, "NEEDLE44")?);
        assert!(cache.regex_matches("needle[0-9]+", &sensitive, "needle44")?);
        assert_eq!(cache.compiled_builds, 2);
        Ok(())
    }

    #[test]
    fn manual_repo_gate_cache_preserves_identity_empty_sets_and_retry_after_error()
    -> Result<(), CoreError> {
        let mut cache = ManualScanCache::default();
        let options = LqOptions::defaults();
        let args = [LqPredicateArg::Keyword("needle".into())];
        let collections = Cell::new(0);
        for _ in 0..3 {
            let ids = cache.repo_gate_ids("repo.has.content", &args, &options, || {
                collections.set(collections.get() + 1);
                Ok(["repo-a".to_string()].into())
            })?;
            assert!(ids.contains("repo-a"));
        }
        assert_eq!(collections.get(), 1);
        let empty = cache.repo_gate_ids("repo.has.file", &args, &options, || {
            collections.set(collections.get() + 1);
            Ok(BTreeSet::new())
        })?;
        assert!(empty.is_empty());
        let empty = cache.repo_gate_ids("repo.has.file", &args, &options, || {
            Err(CoreError::Storage(
                "cached empty gate was recollected".into(),
            ))
        })?;
        assert!(empty.is_empty());
        assert_eq!(collections.get(), 2);
        let other_args = [LqPredicateArg::Keyword("other".into())];
        assert!(
            cache
                .repo_gate_ids("repo.has.content", &other_args, &options, || {
                    Err(CoreError::Storage("transient collection failure".into()))
                })
                .is_err()
        );
        let retry = cache.repo_gate_ids("repo.has.content", &other_args, &options, || {
            collections.set(collections.get() + 1);
            Ok(["repo-b".to_string()].into())
        })?;
        assert!(retry.contains("repo-b"));
        assert_eq!(collections.get(), 3);
        Ok(())
    }

    #[test]
    fn manual_regex_cache_compiles_each_execution_pattern_once_and_does_not_cache_failures()
    -> Result<(), CoreError> {
        let mut cache = ManualScanCache::default();
        let compilations = Cell::new(0);
        for _ in 0..3 {
            let executor = cache.get_or_compile("a+", || {
                compilations.set(compilations.get() + 1);
                TantivySearcher::manual_filter_regex("a+", "file")
            })?;
            assert!(executor.verify(b"aaa"));
        }
        assert_eq!(compilations.get(), 1);
        let insensitive = cache.get_or_compile("(?i)a+", || {
            compilations.set(compilations.get() + 1);
            TantivySearcher::manual_filter_regex("(?i)a+", "file")
        })?;
        assert!(insensitive.verify(b"AAA"));
        assert_eq!(compilations.get(), 2);
        assert!(
            cache
                .get_or_compile("[", || TantivySearcher::manual_filter_regex("[", "file"))
                .is_err()
        );
        assert_eq!(cache.compiled.len(), 2);
        for pattern in ["b+", "c+", "d+"] {
            let _executor = cache.get_or_compile(pattern, || {
                compilations.set(compilations.get() + 1);
                TantivySearcher::manual_filter_regex(pattern, "file")
            })?;
            assert!(cache.compiled.len() <= MANUAL_REGEX_CACHE_ENTRIES);
        }
        assert_eq!(cache.compiled.len(), MANUAL_REGEX_CACHE_ENTRIES);
        // `(?i)a+` was the first lexical key, so deterministic eviction must
        // compile it again when the query revisits that leaf.
        let _executor = cache.get_or_compile("(?i)a+", || {
            compilations.set(compilations.get() + 1);
            TantivySearcher::manual_filter_regex("(?i)a+", "file")
        })?;
        assert_eq!(compilations.get(), 6);
        Ok(())
    }

    #[test]
    fn required_manual_fields_reject_missing_malformed_and_duplicate_values()
    -> Result<(), CoreError> {
        let mut schema = Schema::builder();
        let content = schema.add_text_field("chunk_text", STORED);
        let _schema = schema.build();

        let missing = TantivyDocument::new();
        assert!(required_stored_text(&missing, content, "chunk_text").is_err());

        let mut malformed = TantivyDocument::new();
        malformed.add_u64(content, 7);
        assert!(required_stored_text(&malformed, content, "chunk_text").is_err());

        let mut duplicate = TantivyDocument::new();
        duplicate.add_text(content, "first");
        duplicate.add_text(content, "second");
        assert!(required_stored_text(&duplicate, content, "chunk_text").is_err());

        let mut valid = TantivyDocument::new();
        valid.add_text(content, "needle");
        assert_eq!(
            required_stored_text(&valid, content, "chunk_text")?,
            "needle"
        );
        Ok(())
    }

    #[test]
    fn unknown_document_kind_is_not_silently_excluded() {
        let mut schema = Schema::builder();
        let kind = schema.add_text_field("doc_kind", STORED);
        let _schema = schema.build();
        let mut doc = TantivyDocument::new();
        doc.add_text(kind, "unknown");
        assert!(stored_doc_kind(&doc, kind).is_err());
    }
}
