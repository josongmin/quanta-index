//! Structural request lowering and feature-surface validation.

use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    GenerationPin, LqExpr, LqFilter, LqLeaf, LqQuery, LqStructuralBlock, LqStructuralConstraint,
    LqStructuralConstraintOperand, LqStructuralExpr, LqStructuralHoleRef, LqStructuralNode,
    SearchPlaneTrackKind, StructuralQueryRequest, TextQuerySyntax,
};
use quanta_index_core::CoreError;
use quanta_index_core::domains::structural::StructuralExecutableFilter;

use crate::lowering::lower_sourcegraph_structural_query_text;
use crate::query_dispatcher::errors::structural_invalid_request;
use crate::query_dispatcher::selection::resolve_lexical_request_pin;
use crate::{ActivationCatalog, lower_lexical_text_query};

pub(super) fn lower_structural_query_request(
    activation_catalog: &ActivationCatalog,
    request: &StructuralQueryRequest,
) -> Result<(GenerationPin, LqQuery), CoreError> {
    let lowered = match request.text_query.syntax {
        TextQuerySyntax::Native => lower_lexical_text_query(&request.text_query)?,
        TextQuerySyntax::Sourcegraph => {
            lower_sourcegraph_structural_query_text(&request.text_query.query_text)?
        }
    };
    validate_structural_feature_surface(&lowered)?;
    let pin = resolve_lexical_request_pin(
        activation_catalog,
        &request.text_query,
        SearchPlaneTrackKind::Structural,
        "structural",
    )?;
    Ok((pin, lowered))
}

fn validate_structural_feature_surface(query: &LqQuery) -> Result<(), CoreError> {
    if query.options.timeout_ms.is_some() {
        return Err(structural_invalid_request(
            "structural: timeout option is not executable on the current authority route",
        ));
    }
    reject_typed_structural_holes_in_expr(&query.expr)
}

fn reject_typed_structural_holes_in_expr(expr: &LqExpr) -> Result<(), CoreError> {
    match expr {
        LqExpr::Empty
        | LqExpr::Leaf(
            LqLeaf::Keyword(_)
            | LqLeaf::Phrase(_)
            | LqLeaf::RawString(_)
            | LqLeaf::Regex(_)
            | LqLeaf::Predicate { .. },
        ) => Ok(()),
        LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => {
            reject_typed_structural_holes_in_block(block)
        }
        LqExpr::Not(inner) => reject_typed_structural_holes_in_expr(inner),
        LqExpr::All(children) | LqExpr::Any(children) => {
            for child in children {
                reject_typed_structural_holes_in_expr(child)?;
            }
            Ok(())
        }
    }
}

fn reject_typed_structural_holes_in_block(block: &LqStructuralBlock) -> Result<(), CoreError> {
    for expr in &block.exprs {
        reject_typed_structural_holes_in_structural_expr(expr)?;
    }
    Ok(())
}

fn reject_typed_structural_holes_in_structural_expr(
    expr: &LqStructuralExpr,
) -> Result<(), CoreError> {
    match expr {
        LqStructuralExpr::Pattern(nodes) => {
            for node in nodes {
                reject_typed_structural_holes_in_node(node)?;
            }
            Ok(())
        }
        LqStructuralExpr::Where(constraints) => {
            for constraint in constraints {
                reject_typed_structural_holes_in_constraint(constraint)?;
            }
            Ok(())
        }
        LqStructuralExpr::Inside(block) | LqStructuralExpr::Outside(block) => {
            reject_typed_structural_holes_in_block(block)
        }
    }
}

fn reject_typed_structural_holes_in_node(node: &LqStructuralNode) -> Result<(), CoreError> {
    match node {
        LqStructuralNode::Literal(_)
        | LqStructuralNode::MetaVar(_)
        | LqStructuralNode::WildcardMany => Ok(()),
        LqStructuralNode::Group(children) => {
            for child in children {
                reject_typed_structural_holes_in_node(child)?;
            }
            Ok(())
        }
        LqStructuralNode::Hole { name, multiplicity } => reject_typed_structural_hole_name(
            name.as_ref().map(quanta_index_contract::LqMetaVar::as_str),
            *multiplicity,
        ),
    }
}

fn reject_typed_structural_holes_in_constraint(
    constraint: &LqStructuralConstraint,
) -> Result<(), CoreError> {
    reject_typed_structural_hole_ref(&constraint.left)?;
    if let LqStructuralConstraintOperand::Hole(hole) = &constraint.right {
        reject_typed_structural_hole_ref(hole)?;
    }
    Ok(())
}

fn reject_typed_structural_hole_ref(hole: &LqStructuralHoleRef) -> Result<(), CoreError> {
    reject_typed_structural_hole_name(Some(hole.name.as_str()), hole.multiplicity)
}

fn reject_typed_structural_hole_name(
    name: Option<&str>,
    multiplicity: quanta_index_contract::LqStructuralHoleMultiplicity,
) -> Result<(), CoreError> {
    let Some(name) = name else {
        return Ok(());
    };
    let Some((metavar, kind)) = name.rsplit_once('.') else {
        return Ok(());
    };
    if metavar.is_empty() || kind.is_empty() {
        return Ok(());
    }
    if multiplicity != quanta_index_contract::LqStructuralHoleMultiplicity::One {
        return Err(CoreError::Typed {
            code: LexicalErrorCode::StrHoleKindUnsupported
                .as_code_str()
                .to_string(),
            message: format!(
                "structural typed hole kind `{kind}` is executable only on single-capture holes"
            ),
        });
    }
    if matches!(kind, "expr" | "stmt" | "item" | "type") {
        return Ok(());
    }
    Err(CoreError::Typed {
        code: LexicalErrorCode::StrHoleKindUnsupported
            .as_code_str()
            .to_string(),
        message: format!(
            "structural typed hole kind `{kind}` is not executable on the current authority route"
        ),
    })
}

pub(super) fn extract_structural_filters(
    query: &LqQuery,
    initial_lang: Option<&str>,
) -> Result<(Option<String>, Vec<StructuralExecutableFilter>), CoreError> {
    let mut requested_lang: Option<String> = initial_lang.map(str::to_string);
    let mut executable_filters: Vec<StructuralExecutableFilter> = Vec::new();
    for filter in &query.filters {
        match filter {
            LqFilter::Lang { id } => match requested_lang.as_deref() {
                None => requested_lang = Some(id.clone()),
                Some(current) if current == id => {}
                Some(current) => {
                    return Err(structural_invalid_request(format!(
                        "conflicting lang filters `{current}` and `{id}` are not allowed"
                    )));
                }
            },
            LqFilter::Repo { pattern, revs } => {
                if !revs.is_empty() {
                    return Err(structural_invalid_request(
                        "repo filter revisions are not executable on the current structural adapter set",
                    ));
                }
                executable_filters.push(StructuralExecutableFilter::RepoRegexNoRev {
                    pattern: pattern.clone(),
                });
            }
            LqFilter::File { pattern, scope } => {
                executable_filters.push(StructuralExecutableFilter::FileRegex {
                    pattern: pattern.clone(),
                    scope: *scope,
                });
            }
            other @ (LqFilter::Rev { .. }
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
            | LqFilter::Content { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. }) => {
                return Err(structural_invalid_request(format!(
                    "filter `{}` is not executable on the current structural adapter set",
                    structural_filter_label(other)
                )));
            }
        }
    }
    Ok((requested_lang, executable_filters))
}

fn structural_filter_label(filter: &LqFilter) -> &'static str {
    match filter {
        LqFilter::Repo { .. } => "repo",
        LqFilter::File { .. } => "file",
        LqFilter::Lang { .. } => "lang",
        LqFilter::Rev { .. } => "rev",
        LqFilter::Author { .. } => "author",
        LqFilter::Committer { .. } => "committer",
        LqFilter::Message { .. } => "message",
        LqFilter::Before { .. } => "before",
        LqFilter::After { .. } => "after",
        LqFilter::Since { .. } => "since",
        LqFilter::Until { .. } => "until",
        LqFilter::DiffAdded { .. } => "diff.added",
        LqFilter::DiffRemoved { .. } => "diff.removed",
        LqFilter::DiffTouched { .. } => "diff.touched",
        LqFilter::Type { .. } => "type",
        LqFilter::Select { .. } => "select",
        LqFilter::Dirty { .. } => "dirty",
        LqFilter::Changed { .. } => "changed",
        LqFilter::Stale { .. } => "stale",
        LqFilter::Snapshot { .. } => "snapshot",
        LqFilter::MetaOwner { .. } => "meta.owner",
        LqFilter::MetaService { .. } => "meta.service",
        LqFilter::MetaLayer { .. } => "meta.layer",
        LqFilter::MetaSurface { .. } => "meta.surface",
        LqFilter::Affected { .. } => "affected",
        LqFilter::InvalidatedBy { .. } => "invalidated_by",
        LqFilter::Fork { .. } => "fork",
        LqFilter::Archived { .. } => "archived",
        LqFilter::Content { .. } => "content",
        LqFilter::Visibility { .. } => "visibility",
        LqFilter::Context { .. } => "context",
    }
}
