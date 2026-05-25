use quanta_index_contract::{
    LqExpr, LqFilter, LqLeaf, LqPatternType, LqQuery, LqSelect, LqType, ManifestGeneration,
};

use crate::error::CoreError;

#[derive(Debug, Default, Clone, Copy)]
pub struct LexicalPolicy;

impl LexicalPolicy {
    pub fn validate_query(query: &LqQuery) -> Result<(), CoreError> {
        let has_content_filter = query
            .filters
            .iter()
            .any(|filter| matches!(filter, LqFilter::Content { .. }));
        if matches!(query.expr, LqExpr::Empty) && !has_content_filter {
            return Err(CoreError::InvalidContract(
                "lexical: empty query is rejected (must carry an expression or content filter)"
                    .to_string(),
            ));
        }
        if query.options.pattern_type == LqPatternType::Structural
            || expr_contains_structural(&query.expr)
            || filters_contain_structural(&query.filters)
        {
            return Err(CoreError::Typed {
                code: "STR_PRODUCER_PARSE_TREE_UNAVAILABLE".to_string(),
                message:
                    "lexical: structural execution is fail-closed until producer parse-tree ops land"
                        .to_string(),
            });
        }
        validate_supported_filter_surface(query)?;
        validate_predicate_composition(&query.expr)?;
        Ok(())
    }

    /// Reject activation requests when the generation has not been materialized.
    pub fn validate_query_against_readiness(
        target: ManifestGeneration,
        materialized: Option<ManifestGeneration>,
    ) -> Result<(), CoreError> {
        match materialized {
            Some(active) if active.get() >= target.get() => Ok(()),
            Some(active) => Err(CoreError::NotReady(format!(
                "lexical: requested generation {} but materialized only up to {}",
                target.get(),
                active.get()
            ))),
            None => Err(CoreError::NotReady(
                "lexical: no materialized generation yet".to_string(),
            )),
        }
    }
}

fn expr_contains_structural(expr: &LqExpr) -> bool {
    match expr {
        LqExpr::Empty | LqExpr::SemanticVector { .. } => false,
        LqExpr::Leaf(leaf) => leaf_contains_structural(leaf),
        LqExpr::Not(inner) => expr_contains_structural(inner),
        LqExpr::All(children) | LqExpr::Any(children) => {
            children.iter().any(expr_contains_structural)
        }
    }
}

fn filters_contain_structural(filters: &[LqFilter]) -> bool {
    filters.iter().any(|filter| match filter {
        LqFilter::Content { leaf } => leaf_contains_structural(leaf),
        LqFilter::Repo { .. }
        | LqFilter::File { .. }
        | LqFilter::Lang { .. }
        | LqFilter::Rev { .. }
        | LqFilter::Type { .. }
        | LqFilter::Select { .. }
        | LqFilter::Fork { .. }
        | LqFilter::Archived { .. }
        | LqFilter::Visibility { .. }
        | LqFilter::Context { .. } => false,
    })
}

fn leaf_contains_structural(leaf: &LqLeaf) -> bool {
    matches!(leaf, LqLeaf::StructuralBlock(_))
}

fn validate_supported_filter_surface(query: &LqQuery) -> Result<(), CoreError> {
    for filter in &query.filters {
        match filter {
            LqFilter::Rev { .. } => {
                return Err(CoreError::NotImplemented(
                    "lexical: rev filter is not executable on the current adapter set".to_string(),
                ));
            }
            LqFilter::Type { kind } => match kind {
                LqType::File | LqType::Path | LqType::Symbol => {}
                LqType::Commit | LqType::Diff | LqType::Repo => {
                    return Err(CoreError::NotImplemented(format!(
                        "lexical: type filter `{}` is not executable on the current adapter set",
                        kind.as_str()
                    )));
                }
            },
            LqFilter::Select { dim } => match dim {
                LqSelect::File
                | LqSelect::Path
                | LqSelect::Symbol
                | LqSelect::Content
                | LqSelect::ContentMatch => {}
                LqSelect::Repo => {
                    return Err(CoreError::NotImplemented(
                        "lexical: select filter `repo` is not executable on the current adapter set"
                            .to_string(),
                    ));
                }
            },
            LqFilter::Repo { .. }
            | LqFilter::File { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Content { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => {}
        }
    }
    Ok(())
}

fn validate_predicate_composition(expr: &LqExpr) -> Result<(), CoreError> {
    match expr {
        LqExpr::Empty | LqExpr::SemanticVector { .. } | LqExpr::Leaf(_) => Ok(()),
        LqExpr::All(children) => {
            for child in children {
                validate_predicate_composition(child)?;
            }
            Ok(())
        }
        LqExpr::Any(children) => {
            if children.iter().any(expr_contains_predicate) {
                return Err(CoreError::NotImplemented(
                    "lexical: predicate leaves under OR are not executable on the current adapter set"
                        .to_string(),
                ));
            }
            Ok(())
        }
        LqExpr::Not(inner) => {
            if expr_contains_predicate(inner) {
                return Err(CoreError::NotImplemented(
                    "lexical: predicate leaves under NOT are not executable on the current adapter set"
                        .to_string(),
                ));
            }
            Ok(())
        }
    }
}

fn expr_contains_predicate(expr: &LqExpr) -> bool {
    match expr {
        LqExpr::Leaf(LqLeaf::Predicate { .. }) => true,
        LqExpr::Leaf(_) | LqExpr::Empty | LqExpr::SemanticVector { .. } => false,
        LqExpr::Not(inner) => expr_contains_predicate(inner),
        LqExpr::All(children) | LqExpr::Any(children) => {
            children.iter().any(expr_contains_predicate)
        }
    }
}
