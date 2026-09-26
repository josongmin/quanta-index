//! Pure lexical request admission, independent of snapshot contents.

use quanta_index_contract::{
    LqExpr, LqFilter, LqLeaf, LqOptions, LqPatternType, LqPredicateArg, LqQuery, LqSelect, LqType,
    QueryConstraintSetV1, SearchPlaneErrorCodeV2,
};

use super::service::LexicalPolicy;
use crate::{CoreError, LexicalPredicateFamilyV1, LexicalPredicateV1};

/// The response capability promised by the public entry point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LexicalEndpoint {
    Text,
    Symbol,
}

/// Legal execution-authority/decoder combinations. A Symbol response can never
/// be paired with a Text index domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LexicalPlanKind {
    Text,
    SymbolAsText,
    Symbol,
}

/// Immutable request inputs admitted together with their route and decoder.
/// Only `LexicalPolicy::plan_query` can construct this value.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedLexicalPlan {
    query: LqQuery,
    constraints: QueryConstraintSetV1,
    kind: LexicalPlanKind,
}

impl ValidatedLexicalPlan {
    #[must_use]
    pub const fn query(&self) -> &LqQuery {
        &self.query
    }

    #[must_use]
    pub const fn constraints(&self) -> &QueryConstraintSetV1 {
        &self.constraints
    }

    #[must_use]
    pub const fn kind(&self) -> LexicalPlanKind {
        self.kind
    }

    #[must_use]
    pub const fn executes_symbol_domain(&self) -> bool {
        matches!(
            self.kind,
            LexicalPlanKind::SymbolAsText | LexicalPlanKind::Symbol
        )
    }

    /// Authority needs include nested Symbol predicates even when the result
    /// domain and decoder remain Text.
    #[must_use]
    pub fn uses_symbol_authority(&self) -> bool {
        self.executes_symbol_domain()
            || expr_uses_symbol_authority(&self.query.expr)
            || self.query.filters.iter().any(|filter| {
                matches!(filter, LqFilter::Content { leaf } if leaf_uses_symbol_authority(leaf))
            })
    }
}

impl LexicalPolicy {
    /// Validate pure request rules before any logical-empty or snapshot-result
    /// shortcut. Snapshot capability checks remain the pinned reader's job.
    pub fn plan_query(
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        endpoint: LexicalEndpoint,
    ) -> Result<ValidatedLexicalPlan, CoreError> {
        Self::plan_query_inner(query, constraints, endpoint, false)
    }

    /// Filter an explicit nonempty candidate universe. This is the only
    /// additional authority that permits an empty expression without a path.
    pub fn plan_candidate_filter_query(
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        candidate_ids: &std::collections::BTreeSet<String>,
    ) -> Result<ValidatedLexicalPlan, CoreError> {
        if candidate_ids.is_empty() {
            return Err(CoreError::InvalidContract(
                "candidate-filter query requires a nonempty explicit candidate universe".into(),
            ));
        }
        Self::plan_query_inner(query, constraints, LexicalEndpoint::Text, true)
    }

    fn plan_query_inner(
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        endpoint: LexicalEndpoint,
        candidate_filter: bool,
    ) -> Result<ValidatedLexicalPlan, CoreError> {
        Self::validate_query_inner(
            query,
            candidate_filter || constraints.repo_relative_path_exact.is_some(),
        )?;
        let mut uses_exact_symbol = validate_symbol_arguments(&query.expr)?;
        for filter in &query.filters {
            if let LqFilter::Content { leaf } = filter {
                uses_exact_symbol |= validate_symbol_leaf_arguments(leaf)?;
            }
        }
        let mut symbol_domain = match endpoint {
            LexicalEndpoint::Text => None,
            LexicalEndpoint::Symbol => Some(true),
        };
        // This is the same implicit domain the canonical top-level predicate
        // rewrite adds. Nested symbol predicates retain their own authority.
        if matches!(&query.expr, LqExpr::Leaf(leaf) if leaf_uses_symbol_authority(leaf)) {
            merge_domain(&mut symbol_domain, true)?;
        }
        for filter in &query.filters {
            let next = if let LqFilter::Type { kind } = filter {
                Some(matches!(kind, LqType::Symbol))
            } else if let LqFilter::Select { dim } = filter {
                Some(matches!(dim, LqSelect::Symbol))
            } else {
                None
            };
            if let Some(next) = next {
                merge_domain(&mut symbol_domain, next)?;
            }
        }
        let symbol_domain = symbol_domain.unwrap_or(false);
        if uses_exact_symbol && !symbol_domain {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo,
                message: "lexical: exact symbol predicates require a Symbol domain; an implicit symbol-to-text join is unsupported".into(),
            });
        }
        if Self::unsupported_symbol_query_text(query, &query.expr, symbol_domain) {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo,
                message: "lexical: symbol text supports keyword postings only; phrase, raw substring and regex require an unsupported symbol authority".into(),
            });
        }
        let kind = match (endpoint, symbol_domain) {
            (LexicalEndpoint::Text, false) => LexicalPlanKind::Text,
            (LexicalEndpoint::Text, true) => LexicalPlanKind::SymbolAsText,
            (LexicalEndpoint::Symbol, true) => LexicalPlanKind::Symbol,
            (LexicalEndpoint::Symbol, false) => {
                return Err(CoreError::InvalidContract(
                    "symbol endpoint cannot execute the text domain".into(),
                ));
            }
        };
        Ok(ValidatedLexicalPlan {
            query: query.clone(),
            constraints: constraints.clone(),
            kind,
        })
    }

    /// Shared Symbol text-authority policy, also used on planner-rewritten
    /// expressions. Boolean structure is traversed without evaluating results.
    #[must_use]
    pub fn unsupported_symbol_query_text(
        query: &LqQuery,
        expr: &LqExpr,
        symbol_domain: bool,
    ) -> bool {
        unsupported_symbol_text(expr, &query.options, symbol_domain)
            || query.filters.iter().any(|filter| {
                matches!(filter, LqFilter::Content { leaf }
                    if unsupported_symbol_leaf(leaf, &query.options, symbol_domain))
            })
    }
}

fn leaf_uses_symbol_authority(leaf: &LqLeaf) -> bool {
    matches!(leaf, LqLeaf::Predicate { name, .. } if LexicalPredicateV1::from_canonical_name(name)
        .is_some_and(|predicate| predicate.family() == LexicalPredicateFamilyV1::Symbol))
}

fn validate_symbol_leaf_arguments(leaf: &LqLeaf) -> Result<bool, CoreError> {
    if let LqLeaf::Predicate { name, args } = leaf
        && let Some(predicate) = LexicalPredicateV1::from_canonical_name(name)
    {
        return predicate
            .exact_symbol_name_argument(args)
            .map(|argument| argument.is_some());
    }
    Ok(false)
}

fn validate_symbol_arguments(expr: &LqExpr) -> Result<bool, CoreError> {
    match expr {
        LqExpr::Empty => Ok(false),
        LqExpr::Leaf(leaf) => validate_symbol_leaf_arguments(leaf),
        LqExpr::Not(inner) => validate_symbol_arguments(inner),
        LqExpr::All(children) | LqExpr::Any(children) => {
            let mut exact = false;
            for child in children {
                exact |= validate_symbol_arguments(child)?;
            }
            Ok(exact)
        }
    }
}

fn expr_uses_symbol_authority(expr: &LqExpr) -> bool {
    match expr {
        LqExpr::Empty => false,
        LqExpr::Leaf(leaf) => leaf_uses_symbol_authority(leaf),
        LqExpr::Not(inner) => expr_uses_symbol_authority(inner),
        LqExpr::All(children) | LqExpr::Any(children) => {
            children.iter().any(expr_uses_symbol_authority)
        }
    }
}

fn merge_domain(current: &mut Option<bool>, next: bool) -> Result<(), CoreError> {
    if current.is_some_and(|existing| existing != next) {
        return Err(CoreError::InvalidContract(
            "lexical: incompatible endpoint/type/select document domain".into(),
        ));
    }
    *current = Some(next);
    Ok(())
}

fn unsupported_symbol_text(expr: &LqExpr, options: &LqOptions, symbol_domain: bool) -> bool {
    match expr {
        LqExpr::Empty => false,
        LqExpr::Not(inner) => unsupported_symbol_text(inner, options, symbol_domain),
        LqExpr::All(children) | LqExpr::Any(children) => children
            .iter()
            .any(|child| unsupported_symbol_text(child, options, symbol_domain)),
        LqExpr::Leaf(leaf) => unsupported_symbol_leaf(leaf, options, symbol_domain),
    }
}

fn unsupported_symbol_leaf(leaf: &LqLeaf, options: &LqOptions, symbol_domain: bool) -> bool {
    match leaf {
        LqLeaf::Predicate { name, args } if name == LexicalPredicateV1::SymbolHasName.name() => {
            matches!(
                args.as_slice(),
                [LqPredicateArg::Phrase(_) | LqPredicateArg::RawString(_)]
            ) || options.pattern_type == LqPatternType::Regexp
        }
        LqLeaf::Phrase(_) | LqLeaf::RawString(_) | LqLeaf::Regex(_) => symbol_domain,
        LqLeaf::Keyword(_) => symbol_domain && options.pattern_type == LqPatternType::Regexp,
        LqLeaf::StructuralBlock(_) | LqLeaf::Predicate { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{LqCountBound, LqSpan};

    fn query() -> LqQuery {
        let mut query = LqQuery::empty(LqSpan::eof(0));
        query.expr = LqExpr::Leaf(LqLeaf::Keyword("needle".into()));
        query
    }

    #[test]
    fn endpoint_binds_domain_and_decoder_but_text_can_select_symbols() {
        let constraints = QueryConstraintSetV1::unconstrained();
        let mut query = query();
        assert!(matches!(
            LexicalPolicy::plan_query(&query, &constraints, LexicalEndpoint::Text)
                .map(|plan| plan.kind()),
            Ok(LexicalPlanKind::Text)
        ));
        assert!(matches!(
            LexicalPolicy::plan_query(&query, &constraints, LexicalEndpoint::Symbol)
                .map(|plan| plan.kind()),
            Ok(LexicalPlanKind::Symbol)
        ));
        query.filters.push(LqFilter::Select {
            dim: LqSelect::Symbol,
        });
        assert!(matches!(
            LexicalPolicy::plan_query(&query, &constraints, LexicalEndpoint::Text)
                .map(|plan| plan.kind()),
            Ok(LexicalPlanKind::SymbolAsText)
        ));
        query.filters = vec![LqFilter::Select {
            dim: LqSelect::File,
        }];
        assert!(matches!(
            LexicalPolicy::plan_query(&query, &constraints, LexicalEndpoint::Symbol),
            Err(CoreError::InvalidContract(_))
        ));
    }

    #[test]
    fn zero_count_is_invalid_before_snapshot_planning() {
        let mut query = query();
        query.options.count = Some(LqCountBound::Bounded(0));
        assert!(matches!(
            LexicalPolicy::plan_query(
                &query,
                &QueryConstraintSetV1::unconstrained(),
                LexicalEndpoint::Symbol
            ),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexFilterInvalidCount,
                ..
            })
        ));
    }

    #[test]
    fn unsupported_symbol_branch_cannot_be_hidden_by_boolean_emptiness() {
        let mut query = query();
        query.expr = LqExpr::All(vec![
            LqExpr::Empty,
            LqExpr::Leaf(LqLeaf::Regex("needle".into())),
        ]);
        assert!(matches!(
            LexicalPolicy::plan_query(
                &query,
                &QueryConstraintSetV1::unconstrained(),
                LexicalEndpoint::Symbol
            ),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo,
                ..
            })
        ));
    }

    #[test]
    fn nested_symbol_authority_does_not_change_the_text_decoder() {
        let mut query = query();
        query.expr = LqExpr::All(vec![
            query.expr,
            LqExpr::Leaf(LqLeaf::Predicate {
                name: LexicalPredicateV1::SymbolHasName.name().into(),
                args: vec![LqPredicateArg::Keyword("needle".into())],
            }),
        ]);
        let result = LexicalPolicy::plan_query(
            &query,
            &QueryConstraintSetV1::unconstrained(),
            LexicalEndpoint::Text,
        );
        assert!(
            matches!(result, Ok(plan) if plan.kind() == LexicalPlanKind::Text
            && !plan.executes_symbol_domain() && plan.uses_symbol_authority())
        );
    }
    #[test]
    fn filter_only_plan_requires_explicit_nonempty_candidate_authority() {
        let query = LqQuery::empty(LqSpan::eof(0));
        let constraints = QueryConstraintSetV1::unconstrained();
        let ids = std::collections::BTreeSet::from(["candidate".to_string()]);
        assert!(LexicalPolicy::plan_candidate_filter_query(&query, &constraints, &ids).is_ok());
        assert!(
            LexicalPolicy::plan_candidate_filter_query(
                &query,
                &constraints,
                &std::collections::BTreeSet::new()
            )
            .is_err()
        );
        assert!(LexicalPolicy::plan_query(&query, &constraints, LexicalEndpoint::Text).is_err());
        let mut invalid = query;
        invalid.options.count = Some(LqCountBound::Bounded(0));
        assert!(LexicalPolicy::plan_candidate_filter_query(&invalid, &constraints, &ids).is_err());
    }

    #[test]
    fn exact_symbol_predicates_use_canonical_shape_and_authority_rules() {
        for predicate in [
            LexicalPredicateV1::SymbolLocalNameExact,
            LexicalPredicateV1::SymbolQualifiedNameExact,
        ] {
            let mut query = query();
            let leaf = LqLeaf::Predicate {
                name: predicate.name().into(),
                args: vec![LqPredicateArg::Keyword("needle".into())],
            };
            query.expr = LqExpr::Leaf(leaf.clone());
            let constraints = QueryConstraintSetV1::unconstrained();
            assert!(matches!(
                LexicalPolicy::plan_query(&query, &constraints, LexicalEndpoint::Text),
                Ok(plan) if plan.kind() == LexicalPlanKind::SymbolAsText
                    && plan.uses_symbol_authority()
            ));
            query.expr = LqExpr::All(vec![
                LqExpr::Leaf(leaf),
                LqExpr::Leaf(LqLeaf::Keyword("text".into())),
            ]);
            assert!(matches!(
                LexicalPolicy::plan_query(&query, &constraints, LexicalEndpoint::Text),
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo,
                    ..
                })
            ));
            query.filters.push(LqFilter::Select {
                dim: LqSelect::Symbol,
            });
            assert!(matches!(
                LexicalPolicy::plan_query(&query, &constraints, LexicalEndpoint::Text),
                Ok(plan) if plan.kind() == LexicalPlanKind::SymbolAsText
                    && plan.uses_symbol_authority()
            ));
            query.expr = LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Predicate {
                name: predicate.name().into(),
                args: vec![LqPredicateArg::Keyword(String::new())],
            })));
            assert!(matches!(
                LexicalPolicy::plan_query(&query, &constraints, LexicalEndpoint::Text),
                Err(CoreError::InvalidContract(_))
            ));
        }
    }
}
