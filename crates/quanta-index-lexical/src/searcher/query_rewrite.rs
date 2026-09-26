//! Rewrites a query needs before it is planned: symbol-name predicates as leaves.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::documents::strip_regex_delimiters;
use crate::predicate_registry::{ContentScalarArg, PREDICATE_OWNER, unimplemented_predicate};
use quanta_index_contract::{LqExpr, LqFilter, LqLeaf, LqPredicateArg, LqQuery, LqSelect, LqType};
use quanta_index_core::{CoreError, LexicalPredicateV1};

/// Lower a validated `repo.has.content` scalar into a content leaf, applying the
/// same `/regex/`-delimiter stripping the `ContentLeaf` family uses so a regex
/// content gate stays a regex.
pub(crate) fn content_leaf_from_scalar(arg: &ContentScalarArg) -> LqLeaf {
    match arg {
        ContentScalarArg::Keyword(value) => {
            if let Some(regex) = strip_regex_delimiters(value) {
                return LqLeaf::Regex(regex.to_string());
            }
            LqLeaf::Keyword(value.clone())
        }
        ContentScalarArg::Phrase(value) => LqLeaf::Phrase(value.clone()),
        ContentScalarArg::RawString(value) => {
            if let Some(regex) = strip_regex_delimiters(value) {
                return LqLeaf::Regex(regex.to_string());
            }
            LqLeaf::RawString(value.clone())
        }
        ContentScalarArg::Number(value) => LqLeaf::Keyword(value.to_string()),
    }
}

pub(crate) fn symbol_name_predicate_leaf(args: &[LqPredicateArg]) -> Result<LqLeaf, CoreError> {
    if let Some(value) = crate::symbol::symbol_name_keyword(args) {
        return Ok(LqLeaf::Keyword(value.to_string()));
    }
    match args {
        [LqPredicateArg::Phrase(_) | LqPredicateArg::RawString(_)] => Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo,
            message: "lexical: symbol.has.name supports one keyword argument only; symbol phrase and raw substring authorities are unsupported".to_string(),
        }),
        _ => {
            Err(unimplemented_predicate(format!(
                "lexical: predicate leaf `symbol.has.name` only supports exactly one keyword argument (owner: {PREDICATE_OWNER})"
            )))
        }
    }
}

pub(crate) fn rewrite_symbol_name_predicate_query(
    query: &LqQuery,
) -> Result<Option<LqQuery>, CoreError> {
    let LqExpr::Leaf(LqLeaf::Predicate { name, args }) = &query.expr else {
        return Ok(None);
    };
    if name != LexicalPredicateV1::SymbolHasName.name() {
        return Ok(None);
    }

    let needle = symbol_name_predicate_leaf(args)?;
    let mut rewritten = query.clone();
    rewritten.expr = LqExpr::Leaf(needle);
    if !rewritten.filters.iter().any(|filter| {
        matches!(
            filter,
            LqFilter::Type {
                kind: LqType::Symbol
            } | LqFilter::Select {
                dim: LqSelect::Symbol
            }
        )
    }) {
        rewritten.filters.push(LqFilter::Type {
            kind: LqType::Symbol,
        });
    }
    Ok(Some(rewritten))
}
