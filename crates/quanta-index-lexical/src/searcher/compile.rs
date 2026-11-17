//! Compiling a prepared query into the engine query.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::metadata_normalize::normalize_language;
use crate::predicate_registry::{PREDICATE_OWNER, PredicateKind, kind_of, unimplemented_predicate};
use crate::searcher::authorities::NativeQueryScope;
use crate::{PreparedExecutableQuery, PreparedPredicatePlan, QueryDocKind, TantivySearcher};
use quanta_index_contract::{
    LqExpr, LqFilter, LqLeaf, LqOptions, LqPatternType, LqPredicateArg, LqQuery,
    QueryConstraintSetV1,
};
use quanta_index_core::{CoreError, RequestBudgetV1, timeref::is_rev_at_time_spec};
use std::sync::Arc;
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query};

/// Exact name equality uses dedicated STRING fields and the shared NFC/case policy.
pub(crate) fn exact_symbol_name_query(
    fields: &crate::SchemaFields,
    name: &str,
    args: &[LqPredicateArg],
    options: &quanta_index_contract::LqOptions,
) -> Result<Option<Box<dyn tantivy::query::Query>>, quanta_index_core::CoreError> {
    use crate::normalize::{CaseMode, apply_case, nfc};
    use quanta_index_core::LexicalPredicateV1;
    let Some(predicate) = LexicalPredicateV1::from_canonical_name(name) else {
        return Ok(None);
    };
    let Some(value) = predicate.exact_symbol_name_argument(args)? else {
        return Ok(None);
    };
    let sensitive = options.case_mode() == CaseMode::Sensitive;
    let field = match (predicate, sensitive) {
        (LexicalPredicateV1::SymbolLocalNameExact, true) => fields.symbol_local_name,
        (LexicalPredicateV1::SymbolLocalNameExact, false) => fields.symbol_local_name_folded,
        (LexicalPredicateV1::SymbolQualifiedNameExact, true) => fields.symbol_qualified_name,
        (LexicalPredicateV1::SymbolQualifiedNameExact, false) => {
            fields.symbol_qualified_name_folded
        }
        _ => return Ok(None),
    };
    let normalized = nfc(value);
    let term = apply_case(&normalized, options.case_mode());
    Ok(Some(Box::new(tantivy::query::TermQuery::new(
        tantivy::Term::from_field_text(field, &term),
        tantivy::schema::IndexRecordOption::Basic,
    ))))
}

impl TantivySearcher {
    pub(crate) fn compile_expr(
        &self,
        expr: &LqExpr,
        options: &LqOptions,
        include_path_terms: bool,
        budget: &RequestBudgetV1,
        native: &NativeQueryScope,
    ) -> Result<Box<dyn Query>, CoreError> {
        match expr {
            LqExpr::Empty => Ok(Box::new(AllQuery)),
            LqExpr::Leaf(leaf) => {
                self.compile_leaf(leaf, options, include_path_terms, budget, native)
            }
            LqExpr::All(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((
                        Occur::Must,
                        self.compile_expr(part, options, false, budget, native)?,
                    ));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Any(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((
                        Occur::Should,
                        self.compile_expr(part, options, false, budget, native)?,
                    ));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Not(inner) => {
                let inner_q = self.compile_expr(inner, options, false, budget, native)?;
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
        native: &NativeQueryScope,
    ) -> Result<Box<dyn Query>, CoreError> {
        let members = self.regex_match_set(source, options, budget, native)?;
        Ok(self.authority_restriction_query(members))
    }

    pub(crate) fn compile_leaf(
        &self,
        leaf: &LqLeaf,
        options: &LqOptions,
        include_path_terms: bool,
        budget: &RequestBudgetV1,
        native: &NativeQueryScope,
    ) -> Result<Box<dyn Query>, CoreError> {
        if let LqLeaf::Predicate { name, args } = leaf
            && let Some(exact) =
                exact_symbol_name_query(&self.fields, name, args, options)?
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
                    return self.compile_regex_content_leaf(text, options, budget, native);
                }
                if matches!(leaf, LqLeaf::RawString(_)) {
                    let members = self.raw_substring_match_set(text, options, budget, native)?;
                    return Ok(self.authority_restriction_query(Arc::new(members)));
                }
                self.compile_keyword_leaf(text, options, include_path_terms)
            }
            LqLeaf::Phrase(text) => {
                let members = self.phrase_match_set(text, options, budget, native)?;
                Ok(self.authority_restriction_query(Arc::new(members)))
            }
            LqLeaf::Regex(text) => self.compile_regex_content_leaf(text, options, budget, native),
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
                    let repo_ids = self.collect_repo_ids_for_repo_has_file(
                        &constraint,
                        options,
                        budget,
                        native,
                    )?;
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
                        self.collect_repo_ids_for_repo_has_content(&leaf, options, budget, native)?;
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
                        let members =
                            self.content_predicate_match_set(&constraint, budget, native)?;
                        return Ok(self.authority_restriction_query(Arc::new(members)));
                    }
                    let lowered = self.predicate_content_leaf_from_constraint(&constraint);
                    self.compile_leaf(&lowered, options, false, budget, native)
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
        native: &NativeQueryScope,
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
            LqFilter::Content { leaf } => Ok(Some(
                self.compile_leaf(leaf, options, false, budget, native)?,
            )),
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
        native: &NativeQueryScope,
    ) -> Result<Option<Box<dyn Query>>, CoreError> {
        if prepared.force_empty {
            return Ok(None);
        }
        let include_path_terms = Self::enables_path_term_surface(&prepared.expr, &query.options);
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        if !matches!(prepared.expr, LqExpr::Empty) {
            clauses.push((
                Occur::Must,
                self.compile_expr(
                    &prepared.expr,
                    &query.options,
                    include_path_terms,
                    budget,
                    native,
                )?,
            ));
        }
        for filter in &query.filters {
            if let Some(compiled_filter) =
                self.compile_filter(filter, &query.options, budget, native)?
            {
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
        native: &NativeQueryScope,
    ) -> Result<Option<Box<dyn Query>>, CoreError> {
        if matches!(prepared.expr, LqExpr::Empty) && constraints.repo_relative_path_exact.is_some()
        {
            return Ok(Some(Box::new(AllQuery)));
        }
        self.compile_query_from_prepared(query, prepared, budget, native)
    }

    pub(crate) fn prepare_executable_query(
        &self,
        plan: &quanta_index_core::ValidatedLexicalPlan,
        budget: &RequestBudgetV1,
        native: &NativeQueryScope,
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
        let predicate_plan = self.prepare_predicate_plan(&prepared_query, budget, native)?;
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

#[cfg(test)]
mod l3_exact_indexed_tests {
    #![expect(clippy::panic_in_result_fn, reason = "assertions are test failure reporting")]
    use super::*;
    use quanta_index_contract::LqCase;
    use tantivy::collector::Count;
    use tantivy::{Index, IndexWriter, TantivyDocument, doc};

    #[test]
    fn l3_exact_names_are_separate_whole_string_fields_with_nfc_and_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let fields = crate::SchemaFields::build();
        let index = Index::create_in_ram(fields.schema.clone());
        crate::analyzer::register_analyzers(&index);
        let mut writer: IndexWriter<TantivyDocument> =
            index.writer_with_num_threads(1, 15_000_000)?;
        // Fixed postings are the oracle; no production name extractor builds these.
        for (local, folded, qualified, qualified_folded) in [
            ("Café", "café", "One::Café", "one::café"),
            ("Café", "café", "Two::Café", "two::café"),
            ("CaféExtra", "caféextra", "One::CaféExtra", "one::caféextra"),
            ("Café", "café", "One::Café", "one::café"), // Overload remains a distinct row.
            ("Inner", "inner", "Café::Inner", "café::inner"),
        ] {
            let _op = writer.add_document(doc!(fields.symbol_local_name => local,
                fields.symbol_local_name_folded => folded, fields.symbol_qualified_name => qualified,
                fields.symbol_qualified_name_folded => qualified_folded))?;
        }
        let _commit = writer.commit()?;
        writer.wait_merging_threads()?;
        let searcher = index.reader()?.searcher();
        for (name, value, case, expected) in [
            ("symbol.local_name.exact", "CAFE\u{301}", None, 3),
            (
                "symbol.local_name.exact",
                "Café",
                Some(LqCase::Sensitive),
                3,
            ),
            (
                "symbol.local_name.exact",
                "café",
                Some(LqCase::Sensitive),
                0,
            ),
            ("symbol.local_name.exact", "Caf", None, 0),
            ("symbol.local_name.exact", "One::Café", None, 0),
            ("symbol.qualified_name.exact", "ONE::CAFE\u{301}", None, 2),
            ("symbol.qualified_name.exact", "Café", None, 0),
            ("symbol.qualified_name.exact", "Café::Inner", None, 1),
            ("symbol.local_name.exact", "NoSymbol", None, 0),
        ] {
            let options = LqOptions {
                case,
                ..LqOptions::defaults()
            };
            let query = exact_symbol_name_query(
                &fields,
                name,
                &[LqPredicateArg::Keyword(value.into())],
                &options,
            )?
            .expect("exact");
            assert_eq!(
                searcher.search(query.as_ref(), &Count)?,
                expected,
                "{name} {value}"
            );
        }
        assert!(
            exact_symbol_name_query(
                &fields,
                "symbol.has.name",
                &[LqPredicateArg::Keyword("Caf".into())],
                &LqOptions::defaults()
            )?
            .is_none()
        );
        assert!(
            exact_symbol_name_query(
                &fields,
                "symbol.local_name.exact",
                &[LqPredicateArg::Phrase("Café".into())],
                &LqOptions::defaults()
            )
            .is_err()
        );
        Ok(())
    }
}
