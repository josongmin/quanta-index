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
        if query.options.timeout_ms.is_some() && !query_contains_timeout_executable_surface(query) {
            return Err(CoreError::InvalidContract(
                "lexical: timeout option is executable only for regex-backed lexical queries"
                    .to_string(),
            ));
        }
        validate_supported_filter_surface(query)?;
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
        LqExpr::Empty => false,
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
        | LqFilter::Author { .. }
        | LqFilter::Committer { .. }
        | LqFilter::Message { .. }
        | LqFilter::Type { .. }
        | LqFilter::Select { .. }
        | LqFilter::Dirty { .. }
        | LqFilter::Fork { .. }
        | LqFilter::Archived { .. }
        | LqFilter::Visibility { .. }
        | LqFilter::Context { .. } => false,
    })
}

fn leaf_contains_structural(leaf: &LqLeaf) -> bool {
    matches!(leaf, LqLeaf::StructuralBlock(_))
}

fn query_contains_timeout_executable_surface(query: &LqQuery) -> bool {
    expr_contains_timeout_executable_surface(&query.expr, query.options.pattern_type)
        || query.filters.iter().any(|filter| match filter {
            LqFilter::Content { leaf } => {
                leaf_contains_timeout_executable_surface(leaf, query.options.pattern_type)
            }
            LqFilter::Repo { .. }
            | LqFilter::File { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Rev { .. }
            | LqFilter::Author { .. }
            | LqFilter::Committer { .. }
            | LqFilter::Message { .. }
            | LqFilter::Type { .. }
            | LqFilter::Select { .. }
            | LqFilter::Dirty { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => false,
        })
}

fn expr_contains_timeout_executable_surface(expr: &LqExpr, pattern_type: LqPatternType) -> bool {
    match expr {
        LqExpr::Empty => false,
        LqExpr::Leaf(leaf) => leaf_contains_timeout_executable_surface(leaf, pattern_type),
        LqExpr::Not(inner) => expr_contains_timeout_executable_surface(inner, pattern_type),
        LqExpr::All(children) | LqExpr::Any(children) => children
            .iter()
            .any(|child| expr_contains_timeout_executable_surface(child, pattern_type)),
    }
}

fn leaf_contains_timeout_executable_surface(leaf: &LqLeaf, pattern_type: LqPatternType) -> bool {
    matches!(leaf, LqLeaf::Regex(_))
        || (pattern_type == LqPatternType::Regexp
            && matches!(leaf, LqLeaf::Keyword(_) | LqLeaf::RawString(_)))
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
                | LqSelect::ContentMatch
                | LqSelect::Repo => {}
            },
            LqFilter::Author { .. } => {
                return Err(CoreError::NotImplemented(
                    "lexical: author filter is not executable on the current adapter set"
                        .to_string(),
                ));
            }
            LqFilter::Committer { .. } => {
                return Err(CoreError::NotImplemented(
                    "lexical: committer filter is not executable on the current adapter set"
                        .to_string(),
                ));
            }
            LqFilter::Message { .. } => {
                return Err(CoreError::NotImplemented(
                    "lexical: message filter is not executable on the current adapter set"
                        .to_string(),
                ));
            }
            LqFilter::Dirty { .. } => {
                return Err(CoreError::NotImplemented(
                    "lexical: dirty filter is not executable on the current adapter set"
                        .to_string(),
                ));
            }
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

#[cfg(test)]
mod tests {
    //! `cargo mutants` killed gaps:
    //! - line 16 `&& !has_content_filter` → `&& has_content_filter` (delete `!`)
    //! - line 23 first `||` → `&&` in the structural-rejection clause
    //! - line 24 second `||` → `&&` in the structural-rejection clause
    //! - line 44 `>=` → `<` in `validate_query_against_readiness`
    //!
    //! Each test below exercises an input where the original and mutated
    //! conditions disagree, so the observable `Ok`/`Err` outcome flips.

    use super::*;
    use quanta_index_contract::{
        LQ_VERSION_TAG, LqOptions, LqPatternType, LqQuery, LqSpan, LqStructuralBlock,
    };

    fn make_query(expr: LqExpr, filters: Vec<LqFilter>, options: LqOptions) -> LqQuery {
        LqQuery {
            lq_version: LQ_VERSION_TAG,
            expr,
            filters,
            directives: Vec::new(),
            options,
            source_span: LqSpan::eof(0),
        }
    }

    fn empty_query() -> LqQuery {
        make_query(LqExpr::Empty, Vec::new(), LqOptions::defaults())
    }

    #[test]
    fn empty_expr_without_content_filter_rejected_kills_bang_delete_mutation() {
        // Original: `Empty && !has_content_filter` = true && !false = true → Err.
        // Mutation (delete `!`): true && false = false → Ok. Asserting Err kills it.
        assert!(LexicalPolicy::validate_query(&empty_query()).is_err());
    }

    #[test]
    fn empty_expr_with_content_filter_accepted_kills_bang_delete_mutation_other_side() {
        // Original: true && !true = false → no early return → Ok.
        // Mutation (delete `!`): true && true = true → Err. Asserting Ok kills it.
        let mut q = empty_query();
        q.filters.push(LqFilter::Content {
            leaf: LqLeaf::Keyword("needle".to_string()),
        });
        assert!(LexicalPolicy::validate_query(&q).is_ok());
    }

    #[test]
    fn timeout_without_regex_backing_is_rejected() {
        let mut q = make_query(
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            Vec::new(),
            LqOptions::defaults(),
        );
        q.options.timeout_ms = Some(1);
        let result = LexicalPolicy::validate_query(&q);
        assert!(matches!(result, Err(CoreError::InvalidContract(_))));
    }

    #[test]
    fn timeout_with_regex_backing_is_accepted() {
        let mut q = make_query(
            LqExpr::Leaf(LqLeaf::Regex("needle.*".to_string())),
            Vec::new(),
            LqOptions::defaults(),
        );
        q.options.timeout_ms = Some(0);
        assert!(LexicalPolicy::validate_query(&q).is_ok());
    }

    #[test]
    fn structural_pattern_only_rejected_kills_first_or_to_and_mutation() {
        // Inputs: A=true (pattern_type==Structural), B=false (non-structural
        // expr), C=false (no structural filter).
        // Original: A || B || C = true → Err.
        // Mutation (first `||` → `&&`): (A && B) || C = (true && false) || false = false → Ok.
        let mut opts = LqOptions::defaults();
        opts.pattern_type = LqPatternType::Structural;
        let q = make_query(
            LqExpr::Leaf(LqLeaf::Keyword("anything".to_string())),
            Vec::new(),
            opts,
        );
        let result = LexicalPolicy::validate_query(&q);
        assert!(
            matches!(
                result,
                Err(CoreError::Typed { ref code, .. })
                    if code == "STR_PRODUCER_PARSE_TREE_UNAVAILABLE"
            ),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn structural_filter_only_rejected_kills_second_or_to_and_mutation() {
        // Inputs: A=false, B=false, C=true (structural leaf in a Content filter).
        // Original: false || false || true = true → Err.
        // Mutation (second `||` → `&&`): (A || B) && C = (false || false) && true = false → Ok.
        let q = make_query(
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            vec![LqFilter::Content {
                leaf: LqLeaf::StructuralBlock(LqStructuralBlock {
                    lang: None,
                    nodes: Vec::new(),
                    exprs: Vec::new(),
                }),
            }],
            LqOptions::defaults(),
        );
        let result = LexicalPolicy::validate_query(&q);
        assert!(
            matches!(
                result,
                Err(CoreError::Typed { ref code, .. })
                    if code == "STR_PRODUCER_PARSE_TREE_UNAVAILABLE"
            ),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn readiness_kills_ge_to_lt_mutation_at_equality() {
        let pin = ManifestGeneration::new(11);
        assert!(LexicalPolicy::validate_query_against_readiness(pin, Some(pin)).is_ok());
        assert!(
            LexicalPolicy::validate_query_against_readiness(
                ManifestGeneration::new(11),
                Some(ManifestGeneration::new(12)),
            )
            .is_ok()
        );
        assert!(
            LexicalPolicy::validate_query_against_readiness(
                ManifestGeneration::new(12),
                Some(ManifestGeneration::new(11)),
            )
            .is_err()
        );
        assert!(LexicalPolicy::validate_query_against_readiness(pin, None).is_err());
    }
}
