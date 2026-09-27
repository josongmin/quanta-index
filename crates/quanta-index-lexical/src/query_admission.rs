//! Data-independent primitive admission shared by native and dispatcher paths.
//!
//! A result-dependent Boolean shortcut cannot establish that a literal is
//! valid. Walk every primitive first, using the execution owners, then leave
//! producer availability and scope coverage to the pinned read handle.

use quanta_index_contract::{LqExpr, LqFilter, LqLeaf, LqOptions, LqPatternType, LqQuery};
use quanta_index_core::{CoreError, RequestBudgetV1, ValidatedLexicalPlan};

use crate::TantivySearcher;
use crate::planner::{LexicalPlanner, LexicalPlannerError};
use crate::predicate_registry::{
    ContentScalarArg, ContributorPattern, MetaPattern, PredicateKind, RepoFileMatcher,
    canonicalize_predicate_call, kind_of, parse_content_predicate_constraint,
    parse_content_scalar_arg, parse_file_contributor_arg, parse_repo_description_arg,
    parse_repo_file_matchers, parse_repo_meta_arg, parse_timeref_scalar_arg,
    unimplemented_predicate,
};
use crate::regex::RegexPolicy;
use crate::searcher::planner_errors::{map_planner_error, map_regex_plan_error};
use crate::searcher::query_rewrite::{
    content_leaf_from_scalar, rewrite_symbol_name_predicate_query,
};

impl LexicalPlanner {
    /// Admit every primitive before result-dependent simplification, without
    /// opening a snapshot. Uses the adapter's real regex policy and the same
    /// normalizer, planners and predicate registry as native execution.
    pub fn validate_query_primitives(
        plan: &ValidatedLexicalPlan,
        regex_policy: &RegexPolicy,
        budget: &RequestBudgetV1,
    ) -> Result<(), CoreError> {
        budget.checkpoint("lexical primitive admission")?;
        let rewritten = rewrite_symbol_name_predicate_query(plan.query())?;
        let query = rewritten.as_ref().unwrap_or_else(|| plan.query());
        // Pure filter conflicts only. Availability needs an actual read view.
        let _filters = crate::filters::plan_filters(&query.filters, &query.options)
            .map_err(|err| map_planner_error(&LexicalPlannerError::FilterPlan(err)))?;
        admit_expr(query, &query.expr, regex_policy, budget)?;
        for filter in &query.filters {
            budget.checkpoint("lexical filter primitive admission")?;
            #[expect(
                clippy::wildcard_enum_match_arm,
                reason = "only content and scope regex filters carry engine primitives; other scalar contracts belong to domain/filter planning"
            )]
            match filter {
                LqFilter::Content { leaf } => admit_leaf(query, leaf, regex_policy, budget)?,
                LqFilter::Repo { pattern, .. } => {
                    admit_query_scope_regex(query, pattern, "repo")?;
                }
                LqFilter::File { pattern, .. } => {
                    admit_query_scope_regex(query, pattern, "file")?;
                }
                // Remaining filters have no content primitive. Their scalar
                // contracts are validated by domain/filter planning above.
                _ => {}
            }
        }
        Ok(())
    }
}

fn admit_expr(
    query: &LqQuery,
    expr: &LqExpr,
    regex_policy: &RegexPolicy,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    budget.checkpoint("lexical expression primitive admission")?;
    match expr {
        LqExpr::Empty => Ok(()),
        LqExpr::Leaf(leaf) => admit_leaf(query, leaf, regex_policy, budget),
        LqExpr::Not(inner) => admit_expr(query, inner, regex_policy, budget),
        LqExpr::All(parts) | LqExpr::Any(parts) => {
            for part in parts {
                admit_expr(query, part, regex_policy, budget)?;
            }
            Ok(())
        }
    }
}

fn admit_regex(source: &str, options: &LqOptions, policy: &RegexPolicy) -> Result<(), CoreError> {
    let source = TantivySearcher::regex_source_for_options(source, options);
    let _plan = crate::regex::plan_regex(&source, options, policy).map_err(map_regex_plan_error)?;
    Ok(())
}

fn admit_leaf(
    query: &LqQuery,
    leaf: &LqLeaf,
    regex_policy: &RegexPolicy,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    budget.checkpoint("lexical leaf primitive admission")?;
    match leaf {
        LqLeaf::Regex(text) => return admit_regex(text, &query.options, regex_policy),
        LqLeaf::Keyword(text) | LqLeaf::RawString(text)
            if query.options.pattern_type == LqPatternType::Regexp =>
        {
            return admit_regex(text, &query.options, regex_policy);
        }
        LqLeaf::Keyword(text) => {
            let _tokens = crate::query_errors::text_query_tokens(
                text,
                TantivySearcher::case_mode(&query.options),
            )?;
        }
        LqLeaf::RawString(_)
        | LqLeaf::Phrase(_)
        | LqLeaf::StructuralBlock(_)
        | LqLeaf::Predicate { .. } => {}
    }
    // Preserve planner-owned phrase/raw/predicate validation and typed errors.
    LexicalPlanner::validate_expr(query, &LqExpr::Leaf(leaf.clone()))
        .map_err(|err| map_planner_error(&err))?;
    if let LqLeaf::Predicate { name, args } = leaf {
        admit_predicate(query, name, args, regex_policy, budget)?;
    }
    Ok(())
}

fn admit_predicate(
    query: &LqQuery,
    name: &str,
    args: &[quanta_index_contract::LqPredicateArg],
    regex_policy: &RegexPolicy,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    let Some(canonical) = canonicalize_predicate_call(name, args)
        .map_err(|err| unimplemented_predicate(format!("lexical: {err:?}")))?
    else {
        // Symbol predicates have already been admitted by the planner.
        return Ok(());
    };
    let scalar = match kind_of(canonical.name) {
        Some(PredicateKind::RepoContentGate) => Some(
            parse_content_scalar_arg(&canonical.args)
                .map_err(|err| unimplemented_predicate(format!("lexical: {err:?}")))?,
        ),
        Some(PredicateKind::ContentLeaf) => Some(admit_content_predicate(
            query,
            &canonical.args,
            regex_policy,
            budget,
        )?),
        Some(PredicateKind::RepoFileGate) => {
            admit_repo_file_predicate(query, &canonical.args, regex_policy, budget)?;
            None
        }
        Some(PredicateKind::RepoMetaGate) => {
            let arg = parse_repo_meta_arg(&canonical.args).map_err(predicate_error)?;
            for pattern in std::iter::once(&arg.key).chain(arg.value.as_ref()) {
                if let MetaPattern::Regex(source) = pattern {
                    admit_metadata_regex(source)?;
                }
            }
            None
        }
        Some(PredicateKind::RepoDescriptionGate) => {
            let arg = parse_repo_description_arg(&canonical.args).map_err(predicate_error)?;
            admit_metadata_regex(&arg.pattern)?;
            None
        }
        Some(PredicateKind::FileContributorGate) => {
            let arg = parse_file_contributor_arg(&canonical.args).map_err(predicate_error)?;
            if let ContributorPattern::Regex(source) = arg.contributor {
                admit_metadata_regex(&source)?;
            }
            None
        }
        Some(PredicateKind::RepoCommitRecencyGate) => {
            let arg = parse_timeref_scalar_arg(&canonical.args).map_err(predicate_error)?;
            if quanta_index_core::timeref::parse_search_timeref_ms(&arg.value).is_none() {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryInvalidTimeref,
                    message: format!("history: invalid timeref {:?}", arg.value),
                });
            }
            None
        }
        Some(PredicateKind::RepoTopicGate | PredicateKind::FileOwnerGate) | None => None,
    };
    if let Some(scalar) = scalar {
        admit_leaf(
            query,
            &content_leaf_from_scalar(&scalar),
            regex_policy,
            budget,
        )?;
    }
    Ok(())
}

fn predicate_error(err: impl std::fmt::Debug) -> CoreError {
    unimplemented_predicate(format!("lexical: invalid predicate argument: {err:?}"))
}

fn admit_metadata_regex(source: &str) -> Result<(), CoreError> {
    let _regex =
        quanta_index_lq_regex::RegexExecutor::compile(source).map_err(|err| CoreError::Typed {
            code: crate::query_errors::regex_wire_code(err.code),
            message: format!("lexical: metadata regex {source:?} failed to compile: {err}"),
        })?;
    Ok(())
}

fn admit_query_scope_regex(query: &LqQuery, source: &str, name: &str) -> Result<(), CoreError> {
    if TantivySearcher::uses_unindexed_scan(&query.options) {
        let _executor = TantivySearcher::manual_filter_regex(source, name)?;
        Ok(())
    } else {
        admit_scope_regex(source)
    }
}

fn admit_scope_regex(source: &str) -> Result<(), CoreError> {
    // Indexed scope execution uses Tantivy's grammar. The field ordinal does not
    // affect compilation and this constructs no index or reader.
    let _query =
        tantivy::query::RegexQuery::from_pattern(source, tantivy::schema::Field::from_field_id(0))
            .map_err(|err| {
                CoreError::InvalidContract(format!("lexical: regex filter compile: {err}"))
            })?;
    Ok(())
}

fn admit_content_predicate(
    query: &LqQuery,
    args: &[quanta_index_contract::LqPredicateArg],
    regex_policy: &RegexPolicy,
    budget: &RequestBudgetV1,
) -> Result<ContentScalarArg, CoreError> {
    let constraint = parse_content_predicate_constraint(args).map_err(predicate_error)?;
    if let Some(scope) = &constraint.path_scope {
        admit_query_scope_regex(query, &scope.pattern, "file")?;
    }
    // Path discovery uses Standard options; the lowered result expression
    // additionally uses the caller's options in admit_predicate.
    let scoped = LqQuery {
        options: crate::metadata_normalize::standard_pattern_options(),
        ..query.clone()
    };
    admit_leaf(
        &scoped,
        &content_leaf_from_scalar(&constraint.content),
        regex_policy,
        budget,
    )?;
    Ok(constraint.content)
}

fn admit_repo_file_predicate(
    query: &LqQuery,
    args: &[quanta_index_contract::LqPredicateArg],
    regex_policy: &RegexPolicy,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    let constraint = parse_repo_file_matchers(args).map_err(predicate_error)?;
    for matcher in constraint.matchers {
        budget.checkpoint("lexical repo-file primitive admission")?;
        match matcher {
            RepoFileMatcher::Content(value) => admit_leaf(
                query,
                &content_leaf_from_scalar(&ContentScalarArg::Keyword(value)),
                regex_policy,
                budget,
            )?,
            RepoFileMatcher::Path(pattern) | RepoFileMatcher::Name(pattern) => {
                admit_scope_regex(&pattern)?;
            }
            RepoFileMatcher::Language(_) => {}
        }
    }
    Ok(())
}
