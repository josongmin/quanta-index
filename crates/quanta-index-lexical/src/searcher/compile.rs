//! Compiling a prepared query into the engine query.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::metadata_normalize::normalize_language;
use crate::predicate_registry::{PREDICATE_OWNER, PredicateKind, kind_of, unimplemented_predicate};
use crate::{PreparedExecutableQuery, PreparedPredicatePlan, QueryDocKind, TantivySearcher};
use quanta_index_contract::{
    LqExpr, LqFilter, LqLeaf, LqOptions, LqPatternType, LqQuery, QueryConstraintSetV1,
};
use quanta_index_core::{CoreError, RequestBudgetV1, timeref::is_rev_at_time_spec};
use std::sync::Arc;
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query};

impl TantivySearcher {
    pub(crate) fn compile_expr(
        &self,
        expr: &LqExpr,
        options: &LqOptions,
        include_path_terms: bool,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn Query>, CoreError> {
        match expr {
            LqExpr::Empty => Ok(Box::new(AllQuery)),
            LqExpr::Leaf(leaf) => self.compile_leaf(leaf, options, include_path_terms, budget),
            LqExpr::All(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((
                        Occur::Must,
                        self.compile_expr(part, options, false, budget)?,
                    ));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Any(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((
                        Occur::Should,
                        self.compile_expr(part, options, false, budget)?,
                    ));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Not(inner) => {
                let inner_q = self.compile_expr(inner, options, false, budget)?;
                let clauses: Vec<(Occur, Box<dyn Query>)> =
                    vec![(Occur::Must, Box::new(AllQuery)), (Occur::MustNot, inner_q)];
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
        }
    }

    /// Compile a content-side regex leaf via the LXE-04 planner pipeline.
    ///
    /// All regex-shaped content leaves route here — both the explicit
    /// `LqLeaf::Regex(_)` AST shape AND `LqLeaf::Keyword`/`LqLeaf::RawString`
    /// leaves carrying `LqOptions::pattern_type = LqPatternType::Regexp`.
    /// Routing both shapes through one body keeps the dialect filter,
    /// candidate admission, and typed `LEX_REGEX_*` codes identical
    /// across surface kinds — no second path bypasses the planner.
    ///
    /// Pipeline:
    /// 1. plan the regex with `crate::regex::plan_regex` using the caller's
    ///    real [`LqOptions`] and the adapter-injected
    ///    [`RegexPolicy`]. Typed regex failures (lookbehind, possessive,
    ///    pattern budget) surface as `CoreError::Typed { code: "LEX_REGEX_*",
    ///    .. }` with a stable code per dialect-rejection kind.
    /// 2. prefilter the materialized trigram sidecar through
    ///    `regex_prefilter`, falling back to whole-corpus exact verify only
    ///    when the regex exposes no mandatory literals.
    /// 3. exact-verify every prefiltered authority doc via
    ///    `quanta-index-lq-regex::RegexExecutor`; the verified doc ids are
    ///    the match set (shared through the match cache when the regex has
    ///    no timeout), and the index is restricted to them through one
    ///    bitmap query, nothing built per match.
    ///
    /// Vendor tokens (`RegexExecutor`, Tantivy scan) are kept inside this
    /// method; callers see only typed `CoreError`s and `Box<dyn Query>`.
    pub(crate) fn compile_regex_content_leaf(
        &self,
        source: &str,
        options: &LqOptions,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn Query>, CoreError> {
        let members = self.regex_match_set(source, options, budget)?;
        Ok(self.authority_restriction_query(members))
    }

    pub(crate) fn compile_leaf(
        &self,
        leaf: &LqLeaf,
        options: &LqOptions,
        include_path_terms: bool,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn Query>, CoreError> {
        if let LqLeaf::Predicate { name, args } = leaf
            && let Some(exact) =
                crate::symbol::exact_symbol_name_query(&self.fields, name, args, options)?
        {
            return Ok(exact);
        }
        match leaf {
            LqLeaf::Keyword(text) | LqLeaf::RawString(text) => {
                // Both AST shapes route through the planner-gated regex
                // pipeline when the caller's options pin
                // `LqPatternType::Regexp`; bypassing this would skip the
                // dialect filter, candidate admission, and the
                // typed `LEX_REGEX_*` error codes.
                if options.pattern_type == LqPatternType::Regexp {
                    return self.compile_regex_content_leaf(text, options, budget);
                }
                if matches!(leaf, LqLeaf::RawString(_)) {
                    let members = self.raw_substring_match_set(text, options)?;
                    return Ok(self.authority_restriction_query(Arc::new(members)));
                }
                self.compile_keyword_leaf(text, options, include_path_terms)
            }
            LqLeaf::Phrase(text) => {
                let members = self.phrase_match_set(text, options)?;
                Ok(self.authority_restriction_query(Arc::new(members)))
            }
            LqLeaf::Regex(text) => self.compile_regex_content_leaf(text, options, budget),
            LqLeaf::StructuralBlock(_) => Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::StrProducerParseTreeUnavailable,
                ),
                message: "lexical: structural leaf cannot compile without producer parse-tree ops"
                    .to_string(),
            }),
            LqLeaf::Predicate { name, args } => match kind_of(name) {
                Some(PredicateKind::RepoFileGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let constraint =
                        self.repo_has_file_constraint(&canonical_name, &canonical_args)?;
                    let repo_ids =
                        self.collect_repo_ids_for_repo_has_file(&constraint, options, budget)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    Ok(self.repo_id_restriction_query(&repo_ids))
                }
                Some(PredicateKind::RepoContentGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let leaf = self.repo_content_constraint(&canonical_name, &canonical_args)?;
                    let repo_ids =
                        self.collect_repo_ids_for_repo_has_content(&leaf, options, budget)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    Ok(self.repo_id_restriction_query(&repo_ids))
                }
                Some(PredicateKind::RepoCommitRecencyGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let timeref =
                        self.repo_commit_after_constraint(&canonical_name, &canonical_args)?;
                    let repo_ids = self.collect_repo_ids_for_repo_has_commit_after(&timeref)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    Ok(self.repo_id_restriction_query(&repo_ids))
                }
                Some(PredicateKind::RepoMetaGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let arg = self.repo_meta_constraint(&canonical_name, &canonical_args)?;
                    let repo_ids = self.collect_repo_ids_for_repo_has_meta(&arg)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    Ok(self.repo_id_restriction_query(&repo_ids))
                }
                Some(PredicateKind::FileOwnerGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let arg = self.file_owner_constraint(&canonical_name, &canonical_args)?;
                    let members = self.file_owner_match_set(&arg, budget)?;
                    Ok(self.authority_restriction_query(Arc::new(members)))
                }
                Some(PredicateKind::FileContributorGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let arg = self.file_contributor_constraint(&canonical_name, &canonical_args)?;
                    let members = self.file_contributor_match_set(&arg, budget)?;
                    Ok(self.authority_restriction_query(Arc::new(members)))
                }
                Some(PredicateKind::RepoTopicGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let arg = self.repo_topic_constraint(&canonical_name, &canonical_args)?;
                    let repo_ids = self.collect_repo_ids_for_repo_has_topic(&arg)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    Ok(self.repo_id_restriction_query(&repo_ids))
                }
                Some(PredicateKind::RepoDescriptionGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let arg = self.repo_description_constraint(&canonical_name, &canonical_args)?;
                    let repo_ids = self.collect_repo_ids_for_repo_has_description(&arg)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    Ok(self.repo_id_restriction_query(&repo_ids))
                }
                Some(PredicateKind::ContentLeaf) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let constraint =
                        self.content_predicate_constraint(&canonical_name, &canonical_args)?;
                    if constraint.has_scopes() {
                        let members = self.content_predicate_match_set(&constraint, budget)?;
                        return Ok(self.authority_restriction_query(Arc::new(members)));
                    }
                    let lowered = self.predicate_content_leaf_from_constraint(&constraint);
                    self.compile_leaf(&lowered, options, false, budget)
                }
                None => Err(unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                ))),
            },
        }
    }

    pub(crate) fn compile_filter(
        &self,
        filter: &LqFilter,
        options: &LqOptions,
        budget: &RequestBudgetV1,
    ) -> Result<Option<Box<dyn Query>>, CoreError> {
        match filter {
            LqFilter::Repo { pattern, revs } => {
                if !revs.is_empty() {
                    // Repo `revs:` argument is the history-producer surface;
                    // the planner records `REV_UNAVAILABLE` for top-level
                    // `Rev { .. }` filters, and this defense-in-depth arm
                    // covers the nested case (`Repo { revs: [...] }`).
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::REV_UNAVAILABLE,
                        message: "lexical: repo filter revisions require a history producer"
                            .to_string(),
                    });
                }
                Ok(Some(
                    self.regex_text_query(self.fields.repo_id, pattern.as_str())?,
                ))
            }
            LqFilter::File { pattern, scope } => {
                Ok(Some(self.compile_file_filter(pattern.as_str(), *scope)?))
            }
            LqFilter::Content { leaf } => {
                Ok(Some(self.compile_leaf(leaf, options, false, budget)?))
            }
            LqFilter::Lang { id } => {
                let Some(language) = normalize_language(id.as_str()) else {
                    return Err(CoreError::InvalidContract(
                        "lexical: lang filter value cannot be empty".to_string(),
                    ));
                };
                Ok(Some(self.exact_text_query(self.fields.language, &language)))
            }
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
            // Type/Select are doc-kind routing concerns handled in
            // `prepare_query_for_doc_kind`; they should never reach
            // `compile_filter`. If a future caller bypasses that pipeline,
            // surface `LEX_FILTER_UNROUTED` so the bug is visible rather
            // than emerging as a silent empty result.
            LqFilter::Type { .. } | LqFilter::Select { .. } => Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexFilterUnrouted,
                message: format!(
                    "lexical: type/select filters must be routed through doc-kind preparation, got `{filter:?}`"
                ),
            }),
            LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => Ok(None),
        }
    }

    pub(crate) fn compile_query_from_prepared(
        &self,
        query: &LqQuery,
        prepared: &PreparedPredicatePlan,
        budget: &RequestBudgetV1,
    ) -> Result<Option<Box<dyn Query>>, CoreError> {
        if prepared.force_empty {
            return Ok(None);
        }
        let include_path_terms = Self::enables_path_term_surface(&prepared.expr, &query.options);
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        if !matches!(prepared.expr, LqExpr::Empty) {
            clauses.push((
                Occur::Must,
                self.compile_expr(&prepared.expr, &query.options, include_path_terms, budget)?,
            ));
        }
        for filter in &query.filters {
            if let Some(compiled_filter) = self.compile_filter(filter, &query.options, budget)? {
                clauses.push((Occur::Must, compiled_filter));
            }
        }
        if let Some(files) = prepared.allowed_files.as_ref() {
            clauses.push((Occur::Must, self.source_file_restriction_query(files)));
        }
        if let Some(repo_ids) = prepared.allowed_repo_ids.as_ref() {
            clauses.push((Occur::Must, self.repo_id_restriction_query(repo_ids)));
        }
        if let Some(candidate_ids) = prepared.allowed_candidate_ids.as_ref() {
            clauses.push((Occur::Must, self.candidate_restriction_query(candidate_ids)));
        }
        match clauses.len() {
            0 => Err(CoreError::InvalidContract(
                "lexical: query lowered to zero executable clauses".to_string(),
            )),
            1 => match clauses.into_iter().next() {
                Some((_, only)) => Ok(Some(only)),
                None => Err(CoreError::InvalidContract(
                    "lexical: query lowered to zero executable clauses".to_string(),
                )),
            },
            _ => Ok(Some(Box::new(BooleanQuery::new(clauses)))),
        }
    }

    pub(crate) fn compile_query_with_constraints(
        &self,
        query: &LqQuery,
        prepared: &PreparedPredicatePlan,
        constraints: &QueryConstraintSetV1,
        budget: &RequestBudgetV1,
    ) -> Result<Option<Box<dyn Query>>, CoreError> {
        if matches!(prepared.expr, LqExpr::Empty) && constraints.repo_relative_path_exact.is_some()
        {
            return Ok(Some(Box::new(AllQuery)));
        }
        self.compile_query_from_prepared(query, prepared, budget)
    }

    pub(crate) fn prepare_executable_query(
        &self,
        plan: &quanta_index_core::ValidatedLexicalPlan,
        budget: &RequestBudgetV1,
    ) -> Result<Option<PreparedExecutableQuery>, CoreError> {
        crate::planner::LexicalPlanner::validate_query_primitives(
            plan,
            &self.regex_policy,
            budget,
        )?;
        let (prepared_query, doc_kind) = self.prepare_query_for_plan(plan)?;
        crate::searcher::planner_errors::planner_preflight_expr(
            &prepared_query,
            &prepared_query.expr,
            self.repo_metadata.is_some(),
        )?;
        if quanta_index_core::LexicalPolicy::unsupported_symbol_query_text(
            &prepared_query,
            &prepared_query.expr,
            matches!(doc_kind, QueryDocKind::Symbol),
        ) {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo,
                message: "lexical: symbol text supports keyword postings only; phrase, raw substring and regex require an unsupported symbol authority".to_string(),
            });
        }
        self.validate_symbol_coverage_for_plan(plan, budget)?;
        let predicate_plan = self.prepare_predicate_plan(&prepared_query, budget)?;
        if predicate_plan.force_empty {
            return Ok(None);
        }
        Ok(Some(PreparedExecutableQuery {
            query: prepared_query,
            predicate_plan,
            doc_kind,
        }))
    }
}
