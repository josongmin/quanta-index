//! Structural boolean-tree evaluation with per-leaf memoization.

use std::collections::BTreeSet;

use quanta_index_contract::{
    GenerationSelector, LqExpr, LqLeaf, LqOptions, LqStructuralBlock,
    MAX_STRUCTURAL_WHERE_REGEX_ENGINES_V1,
};
use quanta_index_core::domains::structural::StructuralExecutableFilter;
use quanta_index_core::domains::structural::StructuralQueryRequest as DomainStructuralQueryRequest;
use quanta_index_core::{CoreError, StructuralService};

use crate::lowering::{
    structural_where_regex_count_error, visit_block_where_regexes, visit_expr_where_regexes,
};
use crate::query_dispatcher::errors::structural_invalid_request;
use crate::query_dispatcher::routes::structural::buckets::{
    StructuralCandidateBuckets, bucket_structural_matches, intersect_structural_buckets,
    structural_candidate_scope_ids, subtract_structural_buckets, union_structural_buckets,
};
use crate::query_dispatcher::routes::structural::lexical_leaves::LexicalSubexprEvaluator;
use crate::query_dispatcher::routes::structural::read::StructuralRead;
use crate::query_dispatcher::routes::structural::universe::compile_structural_filter_regex;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct StructuralLeafExecutionKey {
    pattern: LqStructuralBlock,
    requested_lang: Option<String>,
    filters: Vec<StructuralExecutableFilter>,
    candidate_scope: Option<Vec<String>>,
    options: LqOptions,
}

#[expect(
    clippy::disallowed_types,
    reason = "internal structural leaf memo cache is transient and not part of any persisted or external ordering surface"
)]
type StructuralLeafCache =
    std::collections::HashMap<StructuralLeafExecutionKey, StructuralCandidateBuckets>;

#[derive(Default)]
pub(super) struct StructuralEvalContext {
    leaf_cache: StructuralLeafCache,
    short_circuit_regexes: BTreeSet<String>,
}

pub(super) fn structural_expr_has_structural_leaf(expr: &LqExpr) -> bool {
    match expr {
        LqExpr::Leaf(LqLeaf::StructuralBlock(_)) => true,
        LqExpr::Empty | LqExpr::Leaf(_) => false,
        LqExpr::Not(inner) => structural_expr_has_structural_leaf(inner),
        LqExpr::All(children) | LqExpr::Any(children) => {
            children.iter().any(structural_expr_has_structural_leaf)
        }
    }
}

pub(super) fn structural_expr_has_non_structural_leaf(expr: &LqExpr) -> bool {
    match expr {
        LqExpr::Empty | LqExpr::Leaf(LqLeaf::StructuralBlock(_)) => false,
        LqExpr::Leaf(_) => true,
        LqExpr::Not(inner) => structural_expr_has_non_structural_leaf(inner),
        LqExpr::All(children) | LqExpr::Any(children) => {
            children.iter().any(structural_expr_has_non_structural_leaf)
        }
    }
}

pub(super) fn structural_expr_is_pure_negative_root(expr: &LqExpr) -> bool {
    matches!(expr, LqExpr::Not(_))
}

pub(super) fn extract_structural_requested_lang(
    expr: &LqExpr,
    allow_lexical_leaves: bool,
) -> Result<Option<String>, CoreError> {
    let mut requested_lang = None;
    collect_structural_requested_lang(expr, &mut requested_lang, allow_lexical_leaves)?;
    Ok(requested_lang)
}

fn collect_structural_requested_lang(
    expr: &LqExpr,
    requested_lang: &mut Option<String>,
    allow_lexical_leaves: bool,
) -> Result<(), CoreError> {
    match expr {
        LqExpr::Empty => Err(structural_invalid_request(
            "query must include at least one structural `match { ... }` leaf",
        )),
        LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => {
            merge_structural_requested_lang(requested_lang, block.lang.as_deref())
        }
        LqExpr::Leaf(_) => {
            if allow_lexical_leaves {
                Ok(())
            } else {
                Err(structural_invalid_request(
                    "query must lower to a structural-only boolean tree of `match { ... }` leaves",
                ))
            }
        }
        LqExpr::Not(inner) => {
            collect_structural_requested_lang(inner, requested_lang, allow_lexical_leaves)
        }
        LqExpr::All(children) | LqExpr::Any(children) => {
            if children.is_empty() {
                return Err(structural_invalid_request(
                    "query must include at least one structural `match { ... }` leaf",
                ));
            }
            for child in children {
                collect_structural_requested_lang(child, requested_lang, allow_lexical_leaves)?;
            }
            Ok(())
        }
    }
}

fn merge_structural_requested_lang(
    requested_lang: &mut Option<String>,
    candidate_lang: Option<&str>,
) -> Result<(), CoreError> {
    let candidate_lang = candidate_lang
        .map(str::trim)
        .filter(|lang| !lang.is_empty())
        .map(str::to_string);
    match (requested_lang.as_deref(), candidate_lang.as_deref()) {
        (_, None) => Ok(()),
        (None, Some(lang)) => {
            *requested_lang = Some(lang.to_string());
            Ok(())
        }
        (Some(current), Some(lang)) if current == lang => Ok(()),
        (Some(current), Some(lang)) => Err(structural_invalid_request(format!(
            "conflicting structural lang requirements `{current}` and `{lang}` are not allowed"
        ))),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "structural dispatch context; bundling is a separate refactor"
)]
pub(super) fn evaluate_structural_expr(
    ctx: &mut StructuralEvalContext,
    service: &StructuralService,
    read: StructuralRead<'_>,
    expr: &LqExpr,
    requested_lang: Option<&str>,
    filters: &[StructuralExecutableFilter],
    options: &LqOptions,
    seed: Option<&StructuralCandidateBuckets>,
    lexical_eval: Option<&LexicalSubexprEvaluator<'_>>,
) -> Result<StructuralCandidateBuckets, CoreError> {
    match expr {
        LqExpr::Empty => Err(structural_invalid_request(
            "query must include at least one structural `match { ... }` leaf",
        )),
        LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => execute_structural_block(
            ctx,
            service,
            read,
            block,
            requested_lang,
            filters,
            options,
            seed,
        ),
        LqExpr::Leaf(_) => {
            let Some(evaluator) = lexical_eval else {
                return Err(structural_invalid_request(
                    "query must lower to a structural-only boolean tree of `match { ... }` leaves",
                ));
            };
            evaluator.evaluate(expr)
        }
        LqExpr::Not(inner) => {
            let Some(seed) = seed else {
                return Err(structural_invalid_request(
                    "pure-negative structural boolean queries are not executable; add a positive structural leaf before `NOT`",
                ));
            };
            let blocked = evaluate_structural_expr(
                ctx,
                service,
                read,
                inner,
                requested_lang,
                filters,
                options,
                Some(seed),
                lexical_eval,
            )?;
            Ok(subtract_structural_buckets(seed, &blocked))
        }
        LqExpr::All(children) => {
            if children.is_empty() {
                return Err(structural_invalid_request(
                    "query must include at least one structural `match { ... }` leaf",
                ));
            }
            let mut positives = children
                .iter()
                .filter(|child| !matches!(child, LqExpr::Not(_)));
            let mut current = if let Some(first_positive) = positives.next() {
                let mut current = evaluate_structural_expr(
                    ctx,
                    service,
                    read,
                    first_positive,
                    requested_lang,
                    filters,
                    options,
                    seed,
                    lexical_eval,
                )?;
                for child in positives.by_ref() {
                    let next = evaluate_structural_expr(
                        ctx,
                        service,
                        read,
                        child,
                        requested_lang,
                        filters,
                        options,
                        Some(&current),
                        lexical_eval,
                    )?;
                    current = intersect_structural_buckets(&current, &next);
                    if current.is_empty() {
                        for remaining in positives {
                            admit_skipped_structural_regexes(
                                remaining,
                                &mut ctx.short_circuit_regexes,
                            )?;
                        }
                        for negative in children
                            .iter()
                            .filter(|child| matches!(child, LqExpr::Not(_)))
                        {
                            admit_skipped_structural_regexes(
                                negative,
                                &mut ctx.short_circuit_regexes,
                            )?;
                        }
                        return Ok(current);
                    }
                }
                current
            } else if let Some(seed) = seed {
                seed.clone()
            } else {
                return Err(structural_invalid_request(
                    "pure-negative structural boolean queries are not executable; add a positive structural leaf before `NOT`",
                ));
            };
            let mut negatives = children
                .iter()
                .filter(|child| matches!(child, LqExpr::Not(_)));
            for child in negatives.by_ref() {
                current = evaluate_structural_expr(
                    ctx,
                    service,
                    read,
                    child,
                    requested_lang,
                    filters,
                    options,
                    Some(&current),
                    lexical_eval,
                )?;
                if current.is_empty() {
                    for remaining in negatives {
                        admit_skipped_structural_regexes(
                            remaining,
                            &mut ctx.short_circuit_regexes,
                        )?;
                    }
                    return Ok(current);
                }
            }
            Ok(current)
        }
        LqExpr::Any(children) => {
            if children.is_empty() {
                return Err(structural_invalid_request(
                    "query must include at least one structural `match { ... }` leaf",
                ));
            }
            let mut union = StructuralCandidateBuckets::new();
            for child in children {
                let child_matches = evaluate_structural_expr(
                    ctx,
                    service,
                    read,
                    child,
                    requested_lang,
                    filters,
                    options,
                    seed,
                    lexical_eval,
                )?;
                union = union_structural_buckets(union, child_matches);
            }
            Ok(union)
        }
    }
}

fn execute_structural_block(
    ctx: &mut StructuralEvalContext,
    service: &StructuralService,
    read: StructuralRead<'_>,
    block: &LqStructuralBlock,
    requested_lang: Option<&str>,
    filters: &[StructuralExecutableFilter],
    options: &LqOptions,
    seed: Option<&StructuralCandidateBuckets>,
) -> Result<StructuralCandidateBuckets, CoreError> {
    let candidate_scope = seed.map(structural_candidate_scope_ids);
    if candidate_scope.as_ref().is_some_and(Vec::is_empty) {
        admit_short_circuited_where_regexes(block, &mut ctx.short_circuit_regexes)?;
        return Ok(StructuralCandidateBuckets::new());
    }
    let cache_key = StructuralLeafExecutionKey {
        pattern: block.clone(),
        requested_lang: requested_lang.map(str::to_string),
        filters: filters.to_vec(),
        candidate_scope: candidate_scope.clone(),
        options: options.clone(),
    };
    if let Some(cached) = ctx.leaf_cache.get(&cache_key) {
        return Ok(cached.clone());
    }
    let response = service
        .query(&DomainStructuralQueryRequest {
            pattern: block.clone(),
            requested_lang: requested_lang.map(str::to_string),
            filters: filters.to_vec(),
            candidate_scope,
            options: options.clone(),
            generation: GenerationSelector::Pinned(read.pin.clone()),
            aux_epoch: read.epoch,
        })
        .map_err(|err| map_structural_error(&err))?;
    let buckets = bucket_structural_matches(response.candidates);
    let _prior = ctx.leaf_cache.insert(cache_key, buckets.clone());
    Ok(buckets)
}

/// The producer is intentionally skipped for an empty lexical seed. Preserve
/// its regex admission without acquiring a generation or scanning chunks.
fn admit_short_circuited_where_regexes(
    block: &LqStructuralBlock,
    admitted: &mut BTreeSet<String>,
) -> Result<(), CoreError> {
    visit_block_where_regexes(block, &mut |pattern| {
        admit_one_short_circuited_where_regex(pattern, admitted)
    })
}

fn admit_skipped_structural_regexes(
    expr: &LqExpr,
    admitted: &mut BTreeSet<String>,
) -> Result<(), CoreError> {
    visit_expr_where_regexes(expr, &mut |pattern| {
        admit_one_short_circuited_where_regex(pattern, admitted)
    })
}

fn admit_one_short_circuited_where_regex(
    pattern: &str,
    admitted: &mut BTreeSet<String>,
) -> Result<(), CoreError> {
    if admitted.contains(pattern) {
        return Ok(());
    }
    if admitted.len() >= MAX_STRUCTURAL_WHERE_REGEX_ENGINES_V1 {
        return Err(structural_where_regex_count_error());
    }
    // No candidate consumes this engine: validate once, then release it
    // instead of retaining up to eight compiled engines until request end.
    drop(compile_structural_filter_regex("where", pattern)?);
    let _inserted = admitted.insert(pattern.to_owned());
    Ok(())
}

fn map_structural_error(
    err: &quanta_index_core::domains::structural::StructuralError,
) -> CoreError {
    CoreError::Typed {
        code: err.code(),
        message: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::admit_short_circuited_where_regexes;
    use crate::lowering::admit_structural_where_regex_cardinality;
    use quanta_index_contract::{
        LqExpr, LqLeaf, LqMetaVar, LqStructuralBlock, LqStructuralConstraint,
        LqStructuralConstraintOperand, LqStructuralExpr, LqStructuralHoleMultiplicity,
        LqStructuralHoleRef, MAX_STRUCTURAL_WHERE_REGEX_ENGINES_V1, SearchPlaneErrorCodeV2,
    };
    use quanta_index_core::CoreError;
    use std::collections::BTreeSet;

    fn nested_where(pattern: String) -> LqStructuralBlock {
        let name = LqMetaVar::new("name".to_owned());
        LqStructuralBlock {
            lang: None,
            nodes: Vec::new(),
            exprs: vec![LqStructuralExpr::Inside(Box::new(LqStructuralBlock {
                lang: None,
                nodes: Vec::new(),
                exprs: vec![LqStructuralExpr::Where(vec![LqStructuralConstraint {
                    left: LqStructuralHoleRef {
                        name,
                        multiplicity: LqStructuralHoleMultiplicity::One,
                    },
                    right: LqStructuralConstraintOperand::Regex(pattern),
                }])],
            }))],
        }
    }

    #[test]
    fn empty_seed_still_admits_nested_where_regex() {
        for (pattern, expected) in [
            ("[".to_owned(), SearchPlaneErrorCodeV2::StrInvalidRequest),
            (
                "x".repeat(65_537),
                SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
            ),
        ] {
            assert!(matches!(
                admit_short_circuited_where_regexes(&nested_where(pattern), &mut BTreeSet::new()),
                Err(CoreError::Typed { code, .. }) if code == expected
            ));
        }
        let mut admitted = BTreeSet::new();
        let valid = nested_where("^main$".to_owned());
        assert!(admit_short_circuited_where_regexes(&valid, &mut admitted).is_ok());
        assert!(admit_short_circuited_where_regexes(&valid, &mut admitted).is_ok());
        assert_eq!(
            admitted.len(),
            1,
            "repeated empty leaves share one validated pattern identity"
        );

        let mut admitted = BTreeSet::new();
        for index in 0..MAX_STRUCTURAL_WHERE_REGEX_ENGINES_V1 {
            assert!(
                admit_short_circuited_where_regexes(
                    &nested_where(format!("^value{index}$")),
                    &mut admitted,
                )
                .is_ok()
            );
        }
        assert!(matches!(
            admit_short_circuited_where_regexes(
                &nested_where("^one_more$".to_owned()),
                &mut admitted,
            ),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
                ..
            })
        ));
    }

    #[test]
    fn distinct_where_regex_limit_spans_boolean_leaves() {
        let leaves = |extra: Option<&str>| {
            let mut patterns = (0..MAX_STRUCTURAL_WHERE_REGEX_ENGINES_V1)
                .map(|index| format!("^value{index}$"))
                .collect::<Vec<_>>();
            if let Some(extra) = extra {
                patterns.push(extra.to_owned());
            }
            LqExpr::All(
                patterns
                    .into_iter()
                    .map(|pattern| LqExpr::Leaf(LqLeaf::StructuralBlock(nested_where(pattern))))
                    .collect(),
            )
        };
        assert!(admit_structural_where_regex_cardinality(&leaves(None)).is_ok());
        assert!(admit_structural_where_regex_cardinality(&leaves(Some("^value0$"))).is_ok());
        assert!(matches!(
            admit_structural_where_regex_cardinality(&leaves(Some("^ninth$"))),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
                ..
            })
        ));
    }
}
