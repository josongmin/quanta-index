//! The planner preflight and its typed errors.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::query_errors::map_phrase_plan_error;
use quanta_index_contract::{LqExpr, LqQuery};
use quanta_index_core::CoreError;

/// Exact-all callers consume a complete match set for structural Boolean
/// evaluation. A presentation cap cannot be applied inside that set.
pub(crate) fn validate_exact_all_count(query: &LqQuery) -> Result<(), CoreError> {
    if matches!(
        query.options.count,
        Some(quanta_index_contract::LqCountBound::Bounded(_))
    ) {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexFilterInvalidCount,
            message: "exact-all search cannot honor a bounded count".into(),
        });
    }
    Ok(())
}

/// Run the planner pre-flight and surface its outcome as a typed result.
///
/// Returns `Ok(())` if the plan is executable on the live Tantivy rail.
/// Returns `Err(CoreError::Typed { .. })` for typed-unavailable filters
/// (deterministic first-wins ordering) and for planner errors that lower
/// to typed failures (e.g. `count:0` → `LEX_FILTER_INVALID_COUNT`,
/// unsupported NOT/OR/filter combos → `LEX_PLANNER_UNSUPPORTED_*`).
///
/// Ordering rule for `typed_unavailable`: the planner records typed-
/// unavailable filters in the order they appeared in the input
/// `LqQuery::filters`. The executor surfaces the **first** such entry that
/// is not suppressed by adapter-side producer state — this gives
/// operator-facing diagnostics a deterministic single cause rather than a
/// multi-line laundry list.
///
/// Planner [`Unimplemented`](crate::planner::LexicalPlannerError::Unimplemented)
/// predicate shapes retain the registry's `LexPredicateUnimplemented` code;
/// other shapes surface as `CoreError::NotImplemented` carrying the owning
/// follow-up ticket. The planner is the single authority for these IR
/// shapes — there is no silent delegation to a legacy executor.
pub(crate) fn planner_preflight_expr(
    query: &LqQuery,
    expr: &LqExpr,
    has_repo_metadata: bool,
) -> Result<(), CoreError> {
    let filter_plan = crate::filters::plan_filters(&query.filters, &query.options)
        .map_err(|err| map_planner_error(&crate::planner::LexicalPlannerError::FilterPlan(err)))?;
    for entry in &filter_plan.typed_unavailable {
        if is_unavailable_suppressed_by_metadata(entry.code, has_repo_metadata) {
            continue;
        }
        return Err(CoreError::Typed {
            code: entry.code,
            message: entry.reason.to_string(),
        });
    }
    crate::planner::LexicalPlanner::validate_expr(query, expr)
        .map_err(|err| map_planner_error(&err))
}

/// Lower a [`crate::regex::RegexPlannerError`] into a typed [`CoreError`].
///
/// Each variant maps to a stable wire code so the search-plane and producer
/// can attribute regex rejections without parsing free-form text. The
/// `LEX_REGEX_DIALECT_` prefix mirrors the `regex_*` namespace already used
/// elsewhere in the workspace (e.g. `LEX_REGEX_TRIGRAM_INDEX_MISSING`).
pub(crate) fn map_regex_plan_error(err: crate::regex::RegexPlannerError) -> CoreError {
    use crate::regex::RegexPlannerError;
    match err {
        RegexPlannerError::ParseError { source, detail } => CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexDialectParseError,
            message: format!("lexical: regex parse error for {source:?}: {detail}"),
        },
        RegexPlannerError::UnsupportedFeature { feature } => CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexDialectUnsupported,
            message: format!("lexical: regex unsupported feature `{feature}`"),
        },
        RegexPlannerError::UnboundedCandidatePlan {
            estimated_states,
            budget,
        } => CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexBudgetExceeded,
            message: format!(
                "lexical: regex NFA budget exceeded (estimated {estimated_states} states, budget {budget})"
            ),
        },
    }
}

/// Lower a [`crate::planner::LexicalPlannerError`] into a [`CoreError`].
///
/// Every planner error variant surfaces as a typed `CoreError` so the search
/// path never silently runs an unplanned query and never delegates to a
/// legacy executor.
///
/// * Filter-plan failures (`count:0`, conflicting surfaces, unsupported
///   filter combos) lower to `LEX_FILTER_*` typed codes.
/// * Unsupported IR-shape arms (`UnsupportedNotScope`, `UnsupportedOrScope`,
///   `UnsupportedFilterCombo`) lower to `LEX_PLANNER_UNSUPPORTED_*` typed
///   codes — these are stable wire codes for IR shapes the planner has
///   chosen not to lower.
/// * Registry-owned predicate rejections retain `LexPredicateUnimplemented`.
///   Other `Unimplemented { owner_ticket, .. }` errors carry the owning ticket
///   in `CoreError::NotImplemented`.
/// * `PhrasePlan` lowers through [`map_phrase_plan_error`]: the pre-flight
///   tokenizes phrase text with the shared normalizer, so a token-less or
///   over-long phrase is refused here under the same `LEX_TEXT_QUERY_*`
///   codes the keyword path uses, before any executor lowering runs.
/// * `RegexPlan` uses the same dialect codes as the execution-side regex
///   planner, even when preflight rejects before predicate evaluation.
/// * `TrigramPlan` and `SymbolPlan` retain their contract-fault mapping.
pub(crate) fn map_planner_error(err: &crate::planner::LexicalPlannerError) -> CoreError {
    use crate::planner::LexicalPlannerError;
    match err {
        LexicalPlannerError::FilterPlan(fpe) => match fpe {
            crate::filters::FilterPlannerError::InvalidCount { detail } => CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexFilterInvalidCount,
                message: (*detail).to_string(),
            },
            crate::filters::FilterPlannerError::ConflictingResultSurface { detail } => {
                CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::LexFilterConflictingSurface,
                    message: (*detail).to_string(),
                }
            }
            crate::filters::FilterPlannerError::UnsupportedFilterCombo { detail } => {
                CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::LexFilterUnsupportedCombo,
                    message: (*detail).to_string(),
                }
            }
        },
        LexicalPlannerError::UnsupportedNotScope => CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexPlannerUnsupportedNotScope,
            message: "lexical: planner does not yet lower the NOT scope shape".to_string(),
        },
        LexicalPlannerError::UnsupportedOrScope => CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexPlannerUnsupportedOrScope,
            message: "lexical: planner does not yet lower the OR scope shape".to_string(),
        },
        LexicalPlannerError::UnsupportedFilterCombo => CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo,
            message: "lexical: planner does not yet lower this filter combination".to_string(),
        },
        LexicalPlannerError::Unimplemented { node, owner_ticket } => {
            let message =
                format!("lex planner: IR node '{node}' is unimplemented (owner: {owner_ticket})");
            if *owner_ticket == crate::predicate_registry::PREDICATE_OWNER {
                crate::predicate_registry::unimplemented_predicate(message)
            } else {
                CoreError::NotImplemented(message)
            }
        }
        // The pre-flight plans phrase leaves for real (it tokenizes them), so
        // its literal refusals must carry the same typed codes the executor
        // would have produced for the keyword shape of the same text.
        LexicalPlannerError::PhrasePlan(phrase) => map_phrase_plan_error(phrase.clone()),
        LexicalPlannerError::RegexPlan(regex) => map_regex_plan_error(regex.clone()),
        LexicalPlannerError::TrigramPlan(_) | LexicalPlannerError::SymbolPlan(_) => {
            CoreError::InvalidContract(format!("lexical: planner: {err}"))
        }
    }
}

/// Filter the planner's typed-unavailable list against adapter state.
///
/// The planner is stateless — it does not know which producers this
/// particular `TantivySearcher` actually has wired. The
/// repo-metadata-dependent codes (FORK/ARCHIVED/VISIBILITY/CONTEXT) drop
/// out of the typed-unavailable surface when the adapter has loaded a
/// repo metadata from the bundle payload, because the live
/// `repo_filter_matches` path then handles those filters correctly.
///
/// The `HISTORY_PRODUCER_UNAVAILABLE` and `REV_UNAVAILABLE` codes are
/// never suppressed: no commit/diff/repo producer or history producer is
/// wired on any current configuration of the lexical rail.
pub(crate) fn is_unavailable_suppressed_by_metadata(
    code: quanta_index_contract::SearchPlaneErrorCodeV2,
    has_repo_metadata: bool,
) -> bool {
    if !has_repo_metadata {
        return false;
    }
    matches!(
        code,
        crate::filters::codes::FORK_UNAVAILABLE
            | crate::filters::codes::ARCHIVED_UNAVAILABLE
            | crate::filters::codes::VISIBILITY_UNAVAILABLE
            | crate::filters::codes::CONTEXT_UNAVAILABLE
    )
}
