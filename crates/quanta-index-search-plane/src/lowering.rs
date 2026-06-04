use quanta_index_contract::{
    LqExpr, LqLeaf, LqMetaVar, LqPatternType, LqQuery, LqStructuralBlock, LqStructuralConstraint,
    LqStructuralConstraintOperand, LqStructuralExpr, LqStructuralHoleMultiplicity,
    LqStructuralHoleRef, LqStructuralNode, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_lq_bridge::{
    BridgeError, BridgeErrorCode, SgFilter, SgQuery, SourcegraphVersionTag, parse_sourcegraph,
    translate_query,
};
use quanta_index_lq_norm::{
    LqParseError, LqParseErrorCode, normalizer::normalize, parser::parse, tokenizer::tokenize,
};
use quanta_index_lq_regex::RegexExecutor;

use quanta_index_core::CoreError;

pub fn lower_lexical_text_query(request: &TextQueryRequest) -> Result<LqQuery, CoreError> {
    match request.syntax {
        TextQuerySyntax::Native => lower_lq_query_text(&request.query_text),
        TextQuerySyntax::Sourcegraph => lower_sourcegraph_query_text(&request.query_text),
    }
}

pub fn lower_sourcegraph_query_text(query_text: &str) -> Result<LqQuery, CoreError> {
    lower_sourcegraph_query_text_for_route(query_text, SourcegraphLoweringRoute::Lexical)
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private lowering helpers are shared across sibling modules"
)]
pub(crate) fn lower_sourcegraph_structural_query_text(
    query_text: &str,
) -> Result<LqQuery, CoreError> {
    lower_sourcegraph_query_text_for_route(query_text, SourcegraphLoweringRoute::Structural)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourcegraphLoweringRoute {
    Lexical,
    Structural,
}

fn lower_sourcegraph_query_text_for_route(
    query_text: &str,
    route: SourcegraphLoweringRoute,
) -> Result<LqQuery, CoreError> {
    let sourcegraph = parse_sourcegraph(query_text).map_err(|err| map_bridge_error(&err))?;
    let (sourcegraph, saw_structural_patterntype) = match route {
        SourcegraphLoweringRoute::Lexical => (sourcegraph, false),
        SourcegraphLoweringRoute::Structural => {
            strip_sourcegraph_structural_patterntype(sourcegraph)
                .map_err(|err| map_bridge_error(&err))?
        }
    };
    let version = SourcegraphVersionTag::supported().map_err(|err| map_bridge_error(&err))?;
    let mut query = translate_query(sourcegraph, &version, query_text.len())
        .map_err(|err| map_bridge_error(&err))?;
    if saw_structural_patterntype {
        query.options.pattern_type = LqPatternType::Structural;
    }
    match route {
        SourcegraphLoweringRoute::Lexical => reject_sourcegraph_structural_lexical_shape(query),
        SourcegraphLoweringRoute::Structural => {
            if !saw_structural_patterntype {
                return Err(CoreError::Typed {
                    code: BridgeErrorCode::BridgeTranslateFail
                        .as_code_str()
                        .to_string(),
                    message:
                        "bridge: Sourcegraph structural route requires `patterntype:structural`"
                            .to_string(),
                });
            }
            lower_sourcegraph_structural_shape(query_text, query)
        }
    }
}

fn strip_sourcegraph_structural_patterntype(
    query: SgQuery,
) -> Result<(SgQuery, bool), BridgeError> {
    match query {
        SgQuery::Pattern { .. } | SgQuery::Predicate { .. } => Ok((query, false)),
        SgQuery::And(children) => {
            let mut lowered = Vec::with_capacity(children.len());
            let mut saw = false;
            for child in children {
                let (child, child_saw) = strip_sourcegraph_structural_patterntype(child)?;
                lowered.push(child);
                saw |= child_saw;
            }
            Ok((SgQuery::And(lowered), saw))
        }
        SgQuery::Or(children) => {
            let mut lowered = Vec::with_capacity(children.len());
            let mut saw = false;
            for child in children {
                let (child, child_saw) = strip_sourcegraph_structural_patterntype(child)?;
                lowered.push(child);
                saw |= child_saw;
            }
            Ok((SgQuery::Or(lowered), saw))
        }
        SgQuery::Not(inner) => {
            let (inner, saw) = strip_sourcegraph_structural_patterntype(*inner)?;
            Ok((SgQuery::Not(Box::new(inner)), saw))
        }
        SgQuery::Filtered { filters, body } => {
            let mut kept_filters = Vec::with_capacity(filters.len());
            let mut saw = false;
            for filter in filters {
                match filter {
                    SgFilter::Patterntype(value) if value.as_ref() == "structural" => {
                        saw = true;
                    }
                    SgFilter::Patterntype(value) => {
                        return Err(BridgeError::translate_fail(format!(
                            "bridge: Sourcegraph structural route requires `patterntype:structural`, got `patterntype:{value}`"
                        )));
                    }
                    other @ (SgFilter::Repo(_)
                    | SgFilter::File(_)
                    | SgFilter::Path(_)
                    | SgFilter::Lang(_)
                    | SgFilter::Rev(_)
                    | SgFilter::Author(_)
                    | SgFilter::Committer(_)
                    | SgFilter::Message(_)
                    | SgFilter::Type(_)
                    | SgFilter::Case(_)
                    | SgFilter::Select(_)
                    | SgFilter::Count(_)
                    | SgFilter::Dirty(_)
                    | SgFilter::Changed(_)
                    | SgFilter::Stale(_)
                    | SgFilter::Snapshot(_)
                    | SgFilter::MetaOwner(_)
                    | SgFilter::MetaService(_)
                    | SgFilter::MetaLayer(_)
                    | SgFilter::MetaSurface(_)
                    | SgFilter::Affected(_)
                    | SgFilter::InvalidatedBy(_)
                    | SgFilter::Fork(_)
                    | SgFilter::Archived(_)
                    | SgFilter::Content(_)
                    | SgFilter::Visibility(_)
                    | SgFilter::Context(_)
                    | SgFilter::Index(_)
                    | SgFilter::Boost(_)
                    | SgFilter::Timeout(_)
                    | SgFilter::Before(_)
                    | SgFilter::After(_)
                    | SgFilter::Since(_)
                    | SgFilter::Until(_)
                    | SgFilter::DiffAdded(_)
                    | SgFilter::DiffRemoved(_)
                    | SgFilter::DiffTouched(_)) => kept_filters.push(other),
                }
            }
            let (body, body_saw) = strip_sourcegraph_structural_patterntype(*body)?;
            saw |= body_saw;
            if kept_filters.is_empty() {
                Ok((body, saw))
            } else {
                Ok((
                    SgQuery::Filtered {
                        filters: kept_filters,
                        body: Box::new(body),
                    },
                    saw,
                ))
            }
        }
    }
}

fn lower_lq_query_text(query_text: &str) -> Result<LqQuery, CoreError> {
    let tokens = tokenize(query_text).map_err(|err| map_lq_error(&err))?;
    let parsed = parse(&tokens, query_text).map_err(|err| map_lq_error(&err))?;
    normalize(parsed).map_err(|err| map_lq_error(&err))
}

fn reject_sourcegraph_structural_lexical_shape(query: LqQuery) -> Result<LqQuery, CoreError> {
    if query.options.pattern_type != LqPatternType::Structural {
        return Ok(query);
    }
    Err(CoreError::Typed {
        code: BridgeErrorCode::BridgeTranslateFail
            .as_code_str()
            .to_string(),
        message:
            "bridge: patterntype:structural is not executable on the lexical Sourcegraph route; use the structural route instead"
                .to_string(),
    })
}

fn lower_sourcegraph_structural_shape(
    query_text: &str,
    mut query: LqQuery,
) -> Result<LqQuery, CoreError> {
    if query.options.pattern_type != LqPatternType::Structural {
        return Err(CoreError::Typed {
            code: BridgeErrorCode::BridgeTranslateFail
                .as_code_str()
                .to_string(),
            message: "bridge: Sourcegraph structural route requires `patterntype:structural`"
                .to_string(),
        });
    }
    query.expr = rewrite_sourcegraph_structural_expr(query_text, &query.expr)?;
    Ok(query)
}

/// Per-leaf legality verdict on the Sourcegraph structural route.
///
/// The body-carrying variants hand the borrowed leaf body straight to the
/// rewrite so the rewrite never has to re-match the leaf to recover it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StructuralLeafVerdict<'a> {
    /// Lexical sibling the active LQ wire can represent unchanged (`Keyword`,
    /// `RawString`, `Predicate`). Predicate leaves still pass through the SG
    /// structural family gate in [`structural_route_supports_predicate`]
    /// before they are preserved.
    PreserveLexical,
    /// Phrase body to lower into a structural block.
    LowerPhraseBody(&'a str),
    /// Regex body to lower into a structural block.
    LowerRegexBody(&'a str),
    /// Leaf kind the structural route does not represent (`StructuralBlock`, a
    /// pre-SG shape that should never reach this rewrite).
    TypedFail,
}

/// Leaf-kind legality matrix for the SG structural route. This is the single
/// authority for which leaf kinds the route preserves, lowers, or rejects.
/// Flipping a cell here widens or narrows the mixed-domain subset and must
/// travel with parity proof per the ADV-02 admission bar (enforced by
/// `sourcegraph_structural_leaf_verdict_matrix_is_frozen` and the
/// `check-dsl-capability-truth` gate).
pub(crate) fn structural_leaf_verdict(leaf: &LqLeaf) -> StructuralLeafVerdict<'_> {
    match leaf {
        LqLeaf::Keyword(_) | LqLeaf::RawString(_) | LqLeaf::Predicate { .. } => {
            StructuralLeafVerdict::PreserveLexical
        }
        LqLeaf::Phrase(body) => StructuralLeafVerdict::LowerPhraseBody(body),
        LqLeaf::Regex(body) => StructuralLeafVerdict::LowerRegexBody(body),
        LqLeaf::StructuralBlock(_) => StructuralLeafVerdict::TypedFail,
    }
}

fn structural_route_supports_predicate(name: &str) -> bool {
    matches!(
        name,
        "repo.has.file" | "repo.has.path" | "repo.has.content" | "repo.contains.content"
    )
}

fn structural_route_unsupported_predicate(name: &str) -> CoreError {
    CoreError::Typed {
        code: BridgeErrorCode::BridgeTranslateFail
            .as_code_str()
            .to_string(),
        message: format!(
            "bridge: Sourcegraph structural route preserves only repo gate predicates in mixed boolean cells; `{name}` is unsupported"
        ),
    }
}

/// Typed-fail for a leaf kind the SG structural route does not represent.
fn structural_route_typed_fail() -> CoreError {
    CoreError::Typed {
        code: BridgeErrorCode::BridgeTranslateFail
            .as_code_str()
            .to_string(),
        message:
            "bridge: Sourcegraph structural route accepts only structural pattern bodies plus executable filters"
                .to_string(),
    }
}

/// Rewrite a Sourcegraph structural expression against the leaf-kind legality
/// matrix ([`structural_leaf_verdict`]). Mixed-domain lexical siblings that the
/// active LQ wire can represent (`Keyword`, `RawString`, supported repo-gate
/// `Predicate` families) are preserved unchanged to mirror native execution;
/// `Phrase` / `Regex` bodies become structural blocks; `StructuralBlock`
/// typed-fails. Native execution stays the source of truth — SG widening only
/// follows it.
fn rewrite_sourcegraph_structural_expr(
    query_text: &str,
    expr: &LqExpr,
) -> Result<LqExpr, CoreError> {
    match expr {
        LqExpr::Empty => Err(CoreError::Typed {
            code: BridgeErrorCode::BridgeTranslateFail
                .as_code_str()
                .to_string(),
            message:
                "bridge: Sourcegraph structural route requires at least one structural pattern body"
                    .to_string(),
        }),
        LqExpr::Leaf(leaf) => match structural_leaf_verdict(leaf) {
            StructuralLeafVerdict::PreserveLexical => {
                if let LqLeaf::Predicate { name, .. } = leaf
                    && !structural_route_supports_predicate(name)
                {
                    return Err(structural_route_unsupported_predicate(name));
                }
                Ok(LqExpr::Leaf(leaf.clone()))
            }
            StructuralLeafVerdict::LowerPhraseBody(body) => Ok(LqExpr::Leaf(
                LqLeaf::StructuralBlock(lower_sourcegraph_structural_body(query_text, body)?),
            )),
            StructuralLeafVerdict::LowerRegexBody(body) => Ok(LqExpr::Leaf(
                LqLeaf::StructuralBlock(lower_sourcegraph_structural_regex_body(body)?),
            )),
            StructuralLeafVerdict::TypedFail => Err(structural_route_typed_fail()),
        },
        LqExpr::Not(inner) => Ok(LqExpr::Not(Box::new(rewrite_sourcegraph_structural_expr(
            query_text, inner,
        )?))),
        LqExpr::All(children) => {
            rewrite_sourcegraph_structural_children(query_text, children, true)
        }
        LqExpr::Any(children) => {
            rewrite_sourcegraph_structural_children(query_text, children, false)
        }
    }
}

fn rewrite_sourcegraph_structural_children(
    query_text: &str,
    children: &[LqExpr],
    all: bool,
) -> Result<LqExpr, CoreError> {
    let mut lowered = Vec::with_capacity(children.len());
    for child in children {
        lowered.push(rewrite_sourcegraph_structural_expr(query_text, child)?);
    }
    Ok(if all {
        LqExpr::All(lowered)
    } else {
        LqExpr::Any(lowered)
    })
}

fn lower_sourcegraph_structural_body(
    query_text: &str,
    structural_body: &str,
) -> Result<LqStructuralBlock, CoreError> {
    let native_structural = lower_lq_query_text(&format!("match {{ {structural_body} }}"))?;
    let LqExpr::Leaf(LqLeaf::StructuralBlock(block)) = native_structural.expr else {
        return Err(CoreError::Typed {
            code: BridgeErrorCode::BridgeTranslateFail
                .as_code_str()
                .to_string(),
            message: format!(
                "bridge: internal lowering failure while rewriting Sourcegraph structural query `{query_text}`"
            ),
        });
    };
    Ok(block)
}

fn lower_sourcegraph_structural_regex_body(
    regex_body: &str,
) -> Result<LqStructuralBlock, CoreError> {
    let _validated_regex = RegexExecutor::compile(regex_body).map_err(|err| CoreError::Typed {
        code: BridgeErrorCode::BridgeTranslateFail
            .as_code_str()
            .to_string(),
        message: format!("bridge: invalid Sourcegraph structural regex body: {err}"),
    })?;
    let capture = structural_regex_capture_name(regex_body);
    let capture_metavar = LqMetaVar::new(capture);
    Ok(LqStructuralBlock {
        lang: None,
        nodes: vec![LqStructuralNode::Hole {
            name: Some(capture_metavar.clone()),
            multiplicity: LqStructuralHoleMultiplicity::One,
        }],
        exprs: vec![
            LqStructuralExpr::Pattern(vec![LqStructuralNode::Hole {
                name: Some(capture_metavar.clone()),
                multiplicity: LqStructuralHoleMultiplicity::One,
            }]),
            LqStructuralExpr::Where(vec![LqStructuralConstraint {
                left: LqStructuralHoleRef {
                    name: capture_metavar,
                    multiplicity: LqStructuralHoleMultiplicity::One,
                },
                right: LqStructuralConstraintOperand::Regex(regex_body.to_string()),
            }]),
        ],
    })
}

fn structural_regex_capture_name(regex_body: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in regex_body.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("__sg_regex_{hash:016x}")
}

fn map_lq_error(err: &LqParseError) -> CoreError {
    let code = match err.code {
        LqParseErrorCode::LimitExceededBytes
        | LqParseErrorCode::LimitExceededDepth
        | LqParseErrorCode::LimitExceededFanout
        | LqParseErrorCode::LimitExceededNfa
        | LqParseErrorCode::LimitExceededStructural
        | LqParseErrorCode::TokenInvalid
        | LqParseErrorCode::ForbiddenSyntax
        | LqParseErrorCode::UnknownFilter
        | LqParseErrorCode::InvalidFilterValue
        | LqParseErrorCode::EmptyQuery
        | LqParseErrorCode::UnclosedQuote
        | LqParseErrorCode::RegexParse
        | LqParseErrorCode::InvalidPatternType
        | LqParseErrorCode::UnsupportedCombo
        | LqParseErrorCode::SyntaxError => "PARSE_FAIL",
    };
    CoreError::Typed {
        code: code.to_string(),
        message: err.to_string(),
    }
}

fn map_bridge_error(err: &BridgeError) -> CoreError {
    CoreError::Typed {
        code: err.code.as_code_str().to_string(),
        message: err.detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        StructuralLeafVerdict, lower_lq_query_text, lower_sourcegraph_query_text,
        lower_sourcegraph_structural_query_text, structural_leaf_verdict,
    };
    use quanta_index_contract::{
        LQ_VERSION_TAG, LqCase, LqCountBound, LqExpr, LqFileScope, LqFilter, LqLeaf, LqPatternType,
        LqPredicateArg, LqSelect, LqSpan, LqStructuralBlock, LqStructuralExpr, LqType,
        LqVisibility, LqYesNoOnly,
    };
    use quanta_index_core::CoreError;
    use quanta_index_lq_bridge::BridgeErrorCode;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn typed_error(err: CoreError) -> Result<(String, String), Box<dyn std::error::Error>> {
        match err {
            CoreError::Typed { code, message } => Ok((code, message)),
            other @ (CoreError::InvalidContract(_)
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => Err(format!("expected typed error, got {other:?}").into()),
        }
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts lowered structure via assert!/assert_eq! macros"
    )]
    fn nested_boolean_lowering_preserves_executable_pattern_structure() -> TestResult {
        let lowered =
            lower_sourcegraph_query_text(r#"alpha AND ("beta gamma" OR NOT /c.*d/) AND omega"#)
                .map_err(|err| -> Box<dyn std::error::Error> {
                    format!("nested boolean lowering must succeed: {err:?}").into()
                })?;

        assert_eq!(lowered.lq_version, LQ_VERSION_TAG);
        assert_eq!(
            lowered.expr,
            LqExpr::All(vec![
                LqExpr::Leaf(LqLeaf::Keyword("alpha".to_string())),
                LqExpr::Any(vec![
                    LqExpr::Leaf(LqLeaf::Phrase("beta gamma".to_string())),
                    LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Regex("c.*d".to_string())))),
                ]),
                LqExpr::Leaf(LqLeaf::Keyword("omega".to_string())),
            ])
        );
        assert!(lowered.filters.is_empty());
        assert_eq!(lowered.options.pattern_type, LqPatternType::Standard);
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts lowered structure via assert!/assert_eq! macros"
    )]
    fn filtered_sourcegraph_body_populates_typed_filters_and_options() -> TestResult {
        let raw = "repo:acme/demo lang:rust case:yes count:25 timeout:5s patterntype:regexp select:content.match needle";
        let lowered =
            lower_sourcegraph_query_text(raw).map_err(|err| -> Box<dyn std::error::Error> {
                format!("sourcegraph lowering must succeed: {err:?}").into()
            })?;

        assert_eq!(lowered.lq_version, LQ_VERSION_TAG);
        assert_eq!(
            lowered.expr,
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string()))
        );
        assert_eq!(
            lowered.filters,
            vec![
                LqFilter::Repo {
                    pattern: "acme/demo".to_string(),
                    revs: Vec::new(),
                },
                LqFilter::Lang {
                    id: "rust".to_string(),
                },
                LqFilter::Select {
                    dim: LqSelect::ContentMatch,
                },
            ]
        );
        assert_eq!(lowered.options.case, Some(LqCase::Sensitive));
        assert_eq!(lowered.options.count, Some(LqCountBound::Bounded(25)));
        assert_eq!(lowered.options.timeout_ms, Some(5_000));
        assert_eq!(lowered.options.pattern_type, LqPatternType::Regexp);
        let raw_len = u32::try_from(raw.len()).map_err(|err| -> Box<dyn std::error::Error> {
            format!("test fixture raw length must fit u32: {err}").into()
        })?;
        assert_eq!(lowered.source_span, LqSpan::eof(raw_len));
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts typed error code+message via assert!/assert_eq! macros"
    )]
    fn sourcegraph_lexical_route_rejects_structural_pattern_type_early() -> TestResult {
        let err = match lower_sourcegraph_query_text(r#"patterntype:structural "function_item""#) {
            Ok(query) => {
                return Err(
                    format!("expected structural SG lexical rejection, got {query:?}").into(),
                );
            }
            Err(err) => err,
        };
        let (code, message) = typed_error(err)?;
        assert_eq!(code, BridgeErrorCode::BridgeTranslateFail.as_code_str());
        assert_eq!(
            message,
            "bridge: patterntype:structural is not executable on the lexical Sourcegraph route; use the structural route instead"
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts lowered structure via assert!/assert_eq! macros"
    )]
    fn sourcegraph_structural_route_rewrites_single_pattern_body_into_structural_leaf() -> TestResult
    {
        let lowered = lower_sourcegraph_structural_query_text(
            r#"repo:acme/demo path:src/lib.rs lang:rust patterntype:structural "function_item { { identifier :[name] } }""#,
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            format!("sourcegraph structural lowering must succeed: {err:?}").into()
        })?;

        assert_eq!(
            lowered.filters,
            vec![
                LqFilter::Repo {
                    pattern: "acme/demo".to_string(),
                    revs: Vec::new(),
                },
                LqFilter::File {
                    pattern: "src/lib.rs".to_string(),
                    scope: LqFileScope::PathOnly,
                },
                LqFilter::Lang {
                    id: "rust".to_string(),
                },
            ]
        );
        if lowered.options.pattern_type != LqPatternType::Structural {
            return Err(format!(
                "expected structural pattern type, got {:?}",
                lowered.options.pattern_type
            )
            .into());
        }
        let LqExpr::Leaf(LqLeaf::StructuralBlock(block)) = lowered.expr else {
            return Err("expected structural leaf after SG structural lowering".into());
        };
        assert_eq!(block.lang, None);
        let Some(LqStructuralExpr::Pattern(nodes)) = block.exprs.first() else {
            return Err("expected one SG structural pattern expr".into());
        };
        if nodes.len() < 2 {
            return Err(
                format!("expected non-trivial SG structural pattern nodes, got {nodes:?}").into(),
            );
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_structural_route_rewrites_regex_body_into_structural_leaf() -> TestResult {
        let lowered =
            lower_sourcegraph_structural_query_text(r"patterntype:structural /^function_item$/")
                .map_err(|err| -> Box<dyn std::error::Error> {
                    format!("sourcegraph structural regex lowering must succeed: {err:?}").into()
                })?;

        let LqExpr::Leaf(LqLeaf::StructuralBlock(block)) = lowered.expr else {
            return Err("expected structural leaf after SG structural regex lowering".into());
        };
        let [
            LqStructuralExpr::Pattern(nodes),
            LqStructuralExpr::Where(constraints),
        ] = block.exprs.as_slice()
        else {
            return Err(format!(
                "expected pattern+where SG structural regex block, got {:?}",
                block.exprs
            )
            .into());
        };
        if !matches!(
            nodes.as_slice(),
            [quanta_index_contract::LqStructuralNode::Hole { .. }]
        ) {
            return Err(format!(
                "expected synthetic single-hole regex structural pattern, got {nodes:?}"
            )
            .into());
        }
        let [constraint] = constraints.as_slice() else {
            return Err(format!(
                "expected exactly one SG structural regex constraint, got {constraints:?}"
            )
            .into());
        };
        if !matches!(
            constraint.right,
            quanta_index_contract::LqStructuralConstraintOperand::Regex(ref regex)
                if regex == "^function_item$"
        ) {
            return Err(format!(
                "expected regex structural constraint, got {:?}",
                constraint.right
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_structural_route_rejects_invalid_regex_body() -> TestResult {
        let err = match lower_sourcegraph_structural_query_text(r"patterntype:structural /(/") {
            Ok(query) => {
                return Err(
                    format!("expected invalid regex structural rejection, got {query:?}").into(),
                );
            }
            Err(err) => err,
        };
        let (code, message) = typed_error(err)?;
        if code != BridgeErrorCode::BridgeTranslateFail.as_code_str() {
            return Err(format!(
                "expected {}, got {code}",
                BridgeErrorCode::BridgeTranslateFail.as_code_str()
            )
            .into());
        }
        if !message.starts_with("bridge: invalid Sourcegraph structural regex body:") {
            return Err(format!("unexpected structural regex rejection message: {message}").into());
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_structural_route_rejects_boolean_pattern_composition() -> TestResult {
        let lowered = lower_sourcegraph_structural_query_text(
            r#"patterntype:structural "function_item" OR patterntype:structural "identifier""#,
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            format!("expected SG structural boolean lowering, got {err:?}").into()
        })?;
        match lowered.expr {
            LqExpr::Any(children) => {
                if children.len() != 2 {
                    return Err(format!(
                        "expected 2 boolean structural children, got {children:?}"
                    )
                    .into());
                }
                for child in children {
                    if !matches!(child, LqExpr::Leaf(LqLeaf::StructuralBlock(_))) {
                        return Err(format!(
                            "expected structural leaf in SG boolean tree, got {child:?}"
                        )
                        .into());
                    }
                }
            }
            other @ (LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::Not(_) | LqExpr::All(_)) => {
                return Err(format!("expected Any structural tree, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_structural_route_rewrites_boolean_not_pattern_composition() -> TestResult {
        let lowered = lower_sourcegraph_structural_query_text(
            r#"patterntype:structural "function_item" AND NOT "trait_item""#,
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            format!("expected SG structural boolean NOT lowering, got {err:?}").into()
        })?;
        match lowered.expr {
            LqExpr::All(children) => {
                let [positive, negated] = children.as_slice() else {
                    return Err(format!(
                        "expected 2 structural boolean children, got {children:?}"
                    )
                    .into());
                };
                if !matches!(positive, LqExpr::Leaf(LqLeaf::StructuralBlock(_))) {
                    return Err(format!(
                        "expected structural leaf for positive SG branch, got {positive:?}"
                    )
                    .into());
                }
                match negated {
                    LqExpr::Not(inner)
                        if matches!(inner.as_ref(), LqExpr::Leaf(LqLeaf::StructuralBlock(_))) => {}
                    other @ (LqExpr::Empty
                    | LqExpr::Leaf(_)
                    | LqExpr::Not(_)
                    | LqExpr::All(_)
                    | LqExpr::Any(_)) => {
                        return Err(format!(
                            "expected structural NOT branch after SG lowering, got {other:?}"
                        )
                        .into());
                    }
                }
            }
            other @ (LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::Not(_) | LqExpr::Any(_)) => {
                return Err(format!(
                    "expected boolean structural tree after SG NOT lowering, got {other:?}"
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_structural_route_preserves_lexical_keyword_in_mixed_boolean_or() -> TestResult {
        let lowered = lower_sourcegraph_structural_query_text(
            r#"patterntype:structural parity_needle_alpha OR "function_item { { identifier :[name] } }""#,
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            format!("expected SG mixed structural OR lowering, got {err:?}").into()
        })?;
        match lowered.expr {
            LqExpr::Any(children) => {
                let [lexical, structural] = children.as_slice() else {
                    return Err(format!("expected 2 mixed OR children, got {children:?}").into());
                };
                if !matches!(lexical, LqExpr::Leaf(LqLeaf::Keyword(body)) if body == "parity_needle_alpha")
                {
                    return Err(format!("expected lexical keyword child, got {lexical:?}").into());
                }
                if !matches!(structural, LqExpr::Leaf(LqLeaf::StructuralBlock(_))) {
                    return Err(
                        format!("expected structural block child, got {structural:?}").into(),
                    );
                }
            }
            other @ (LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::Not(_) | LqExpr::All(_)) => {
                return Err(format!("expected mixed OR tree, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_structural_route_preserves_raw_string_in_mixed_boolean_or() -> TestResult {
        // `file:contains('...')` lowers to an executable RawString leaf; mixed
        // with a structural body it must now survive (ADV-02 widening) as the
        // exact tree native produces for `'parity_raw_needle' OR match { ... }`.
        let lowered = lower_sourcegraph_structural_query_text(
            r#"patterntype:structural file:contains('parity_raw_needle') OR "function_item { { identifier :[name] } }""#,
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            format!("expected SG mixed structural RawString OR lowering, got {err:?}").into()
        })?;
        match lowered.expr {
            LqExpr::Any(children) => {
                let [lexical, structural] = children.as_slice() else {
                    return Err(format!("expected 2 mixed OR children, got {children:?}").into());
                };
                if !matches!(lexical, LqExpr::Leaf(LqLeaf::RawString(body)) if body == "parity_raw_needle")
                {
                    return Err(format!("expected RawString lexical child, got {lexical:?}").into());
                }
                if !matches!(structural, LqExpr::Leaf(LqLeaf::StructuralBlock(_))) {
                    return Err(
                        format!("expected structural block child, got {structural:?}").into(),
                    );
                }
            }
            other @ (LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::Not(_) | LqExpr::All(_)) => {
                return Err(format!("expected mixed OR tree, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_structural_route_preserves_predicate_sibling_in_mixed_boolean() -> TestResult {
        // ADV-02 Predicate sibling: `repo:has.file(...)` lowers to a
        // `LqLeaf::Predicate` that is preserved as a lexical sibling of the
        // structural body — the exact tree native produces for
        // `repo:has.file(...) AND match { ... }`. The lexical executor gates the
        // predicate downstream (parity rail proves SG↔native execution).
        let lowered = lower_sourcegraph_structural_query_text(
            r#"patterntype:structural repo:has.file(path:src/lib.rs) AND "function_item { { identifier :[name] } }""#,
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            format!("expected SG mixed structural Predicate AND lowering, got {err:?}").into()
        })?;
        match lowered.expr {
            LqExpr::All(children) => {
                let [predicate, structural] = children.as_slice() else {
                    return Err(format!("expected 2 mixed AND children, got {children:?}").into());
                };
                if !matches!(
                    predicate,
                    LqExpr::Leaf(LqLeaf::Predicate { name, .. }) if name == "repo.has.file"
                ) {
                    return Err(format!("expected predicate sibling, got {predicate:?}").into());
                }
                if !matches!(structural, LqExpr::Leaf(LqLeaf::StructuralBlock(_))) {
                    return Err(
                        format!("expected structural block child, got {structural:?}").into(),
                    );
                }
            }
            other @ (LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::Not(_) | LqExpr::Any(_)) => {
                return Err(format!("expected mixed AND tree, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_structural_route_preserves_scalar_path_predicate_sibling_in_mixed_boolean()
    -> TestResult {
        let lowered = lower_sourcegraph_structural_query_text(
            r#"patterntype:structural repo:has.file(src/lib.rs) AND "function_item { { identifier :[name] } }""#,
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            format!("expected SG mixed structural scalar-path Predicate AND lowering, got {err:?}")
                .into()
        })?;
        match lowered.expr {
            LqExpr::All(children) => {
                let [predicate, structural] = children.as_slice() else {
                    return Err(format!("expected 2 mixed AND children, got {children:?}").into());
                };
                if !matches!(
                    predicate,
                    LqExpr::Leaf(LqLeaf::Predicate { name, args })
                        if name == "repo.has.file"
                            && matches!(args.as_slice(), [LqPredicateArg::Keyword(v)] if v == "src/lib.rs")
                ) {
                    return Err(format!(
                        "expected scalar-path predicate sibling, got {predicate:?}"
                    )
                    .into());
                }
                if !matches!(structural, LqExpr::Leaf(LqLeaf::StructuralBlock(_))) {
                    return Err(
                        format!("expected structural block child, got {structural:?}").into(),
                    );
                }
            }
            other @ (LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::Not(_) | LqExpr::Any(_)) => {
                return Err(format!("expected mixed AND tree, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_structural_route_rejects_repo_scoped_filter_under_mixed_or() -> TestResult {
        let err = match lower_sourcegraph_structural_query_text(
            r#"repo:repo-e2e patterntype:structural parity_needle_alpha OR "function_item { { identifier :[name] } }""#,
        ) {
            Ok(query) => {
                return Err(
                    format!("expected repo-scoped mixed OR to fail closed, got {query:?}").into(),
                );
            }
            Err(err) => err,
        };
        let (code, message) = typed_error(err)?;
        assert_eq!(code, BridgeErrorCode::BridgeTranslateFail.as_code_str());
        assert_eq!(
            message,
            "bridge: scoped filters under OR/NOT are not representable on the active LQ wire"
        );
        Ok(())
    }

    #[test]
    fn sourcegraph_structural_route_preserves_raw_string_in_mixed_boolean_and_not() -> TestResult {
        let lowered = lower_sourcegraph_structural_query_text(
            r#"patterntype:structural "function_item { { identifier :[name] } }" AND NOT file:contains('parity_raw_needle')"#,
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            format!("expected SG mixed structural RawString AND NOT lowering, got {err:?}")
                .into()
        })?;
        match lowered.expr {
            LqExpr::All(children) => {
                let [structural, not_raw] = children.as_slice() else {
                    return Err(format!("expected 2 mixed AND children, got {children:?}").into());
                };
                if !matches!(structural, LqExpr::Leaf(LqLeaf::StructuralBlock(_))) {
                    return Err(
                        format!("expected structural block child, got {structural:?}").into(),
                    );
                }
                if !matches!(
                    not_raw,
                    LqExpr::Not(inner)
                        if matches!(inner.as_ref(), LqExpr::Leaf(LqLeaf::RawString(body)) if body == "parity_raw_needle")
                ) {
                    return Err(format!("expected NOT RawString child, got {not_raw:?}").into());
                }
            }
            other @ (LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::Not(_) | LqExpr::Any(_)) => {
                return Err(format!("expected mixed AND tree, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_structural_route_preserves_predicate_sibling_in_mixed_boolean_and_not()
    -> TestResult {
        let lowered = lower_sourcegraph_structural_query_text(
            r#"patterntype:structural "function_item { { identifier :[name] } }" AND NOT repo:has.file(src/lib.rs)"#,
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            format!("expected SG mixed structural Predicate AND NOT lowering, got {err:?}")
                .into()
        })?;
        match lowered.expr {
            LqExpr::All(children) => {
                let [structural, not_predicate] = children.as_slice() else {
                    return Err(format!("expected 2 mixed AND children, got {children:?}").into());
                };
                if !matches!(structural, LqExpr::Leaf(LqLeaf::StructuralBlock(_))) {
                    return Err(
                        format!("expected structural block child, got {structural:?}").into(),
                    );
                }
                if !matches!(
                    not_predicate,
                    LqExpr::Not(inner)
                        if matches!(
                            inner.as_ref(),
                            LqExpr::Leaf(LqLeaf::Predicate { name, args })
                                if name == "repo.has.file"
                                    && matches!(args.as_slice(), [LqPredicateArg::Keyword(v)] if v == "src/lib.rs")
                        )
                ) {
                    return Err(
                        format!("expected NOT predicate child, got {not_predicate:?}").into(),
                    );
                }
            }
            other @ (LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::Not(_) | LqExpr::Any(_)) => {
                return Err(format!("expected mixed AND tree, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts typed error code+message via assert!/assert_eq! macros"
    )]
    fn sourcegraph_structural_route_rejects_non_repo_predicate_sibling() -> TestResult {
        let err = match lower_sourcegraph_structural_query_text(
            r#"patterntype:structural symbol:has.name(MyTypeSymbol) AND "function_item { { identifier :[name] } }""#,
        ) {
            Ok(query) => {
                return Err(format!(
                    "expected unsupported predicate sibling failure, got {query:?}"
                )
                .into());
            }
            Err(err) => err,
        };
        let (code, message) = typed_error(err)?;
        assert_eq!(code, BridgeErrorCode::BridgeTranslateFail.as_code_str());
        assert_eq!(
            message,
            "bridge: Sourcegraph structural route preserves only repo gate predicates in mixed boolean cells; `symbol.has.name` is unsupported"
        );
        Ok(())
    }

    #[test]
    fn sourcegraph_structural_route_preserves_lexical_keyword_in_mixed_boolean() -> TestResult {
        let lowered = lower_sourcegraph_structural_query_text(
            r#"patterntype:structural parity_needle_alpha AND "function_item { { identifier :[name] } }""#,
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            format!("expected SG mixed structural lowering, got {err:?}").into()
        })?;
        match lowered.expr {
            LqExpr::All(children) => {
                let [lexical, structural] = children.as_slice() else {
                    return Err(format!("expected 2 mixed children, got {children:?}").into());
                };
                if !matches!(lexical, LqExpr::Leaf(LqLeaf::Keyword(body)) if body == "parity_needle_alpha")
                {
                    return Err(format!("expected lexical keyword child, got {lexical:?}").into());
                }
                if !matches!(structural, LqExpr::Leaf(LqLeaf::StructuralBlock(_))) {
                    return Err(
                        format!("expected structural block child, got {structural:?}").into(),
                    );
                }
            }
            other @ (LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::Not(_) | LqExpr::Any(_)) => {
                return Err(format!("expected mixed AND tree, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts lowered structure via assert!/assert_eq! macros"
    )]
    fn richer_bridge_filters_lower_into_active_lq_contract() -> TestResult {
        let lowered = lower_sourcegraph_query_text(
            "path:src/lib.rs rev:refs/heads/main fork:only archived:no visibility:private context:team-search needle",
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            format!("richer bridge filter lowering must succeed: {err:?}").into()
        })?;

        assert_eq!(
            lowered.expr,
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string()))
        );
        assert_eq!(
            lowered.filters,
            vec![
                LqFilter::File {
                    pattern: "src/lib.rs".to_string(),
                    scope: LqFileScope::PathOnly,
                },
                LqFilter::Rev {
                    spec: "refs/heads/main".to_string(),
                },
                LqFilter::Fork {
                    mode: LqYesNoOnly::Only,
                },
                LqFilter::Archived {
                    mode: LqYesNoOnly::No,
                },
                LqFilter::Visibility {
                    mode: LqVisibility::Private,
                },
                LqFilter::Context {
                    name: "team-search".to_string(),
                },
            ]
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts lowered structure via assert!/assert_eq! macros"
    )]
    fn sourcegraph_rev_filter_lowers_into_typed_rev_filter() -> TestResult {
        let lowered = lower_sourcegraph_query_text("type:commit rev:refs/heads/main fix").map_err(
            |err| -> Box<dyn std::error::Error> {
                format!("sourcegraph rev lowering must succeed: {err:?}").into()
            },
        )?;

        assert_eq!(
            lowered.expr,
            LqExpr::Leaf(LqLeaf::Keyword("fix".to_string()))
        );
        assert_eq!(
            lowered.filters,
            vec![
                LqFilter::Type {
                    kind: LqType::Commit,
                },
                LqFilter::Rev {
                    spec: "refs/heads/main".to_string(),
                },
            ]
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts lowered structure via assert!/assert_eq! macros"
    )]
    fn normalized_visibility_bridge_payloads_map_to_fork_and_archived_filters() -> TestResult {
        let lowered = lower_sourcegraph_query_text(
            "visibility:include_forks visibility:only_archived needle",
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            format!("normalized visibility lowering must succeed: {err:?}").into()
        })?;

        assert_eq!(
            lowered.filters,
            vec![
                LqFilter::Fork {
                    mode: LqYesNoOnly::Yes,
                },
                LqFilter::Archived {
                    mode: LqYesNoOnly::Only,
                },
            ]
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts active predicate leaf lowering via assert!/assert_eq! macros"
    )]
    fn repo_predicates_lower_to_active_predicate_leaves() -> TestResult {
        let lowered =
            lower_sourcegraph_query_text(r#"repo:has.file(path:src/lib.rs, name:"Cargo.toml")"#)
                .map_err(|err| -> Box<dyn std::error::Error> {
                    format!("repo predicate lowering must succeed: {err:?}").into()
                })?;

        assert_eq!(
            lowered.expr,
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "repo.has.file".to_string(),
                args: vec![
                    LqPredicateArg::Filter {
                        name: "path".to_string(),
                        value: "src/lib.rs".to_string(),
                    },
                    LqPredicateArg::Filter {
                        name: "name".to_string(),
                        value: "\"Cargo.toml\"".to_string(),
                    },
                ],
            })
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts active predicate leaf lowering via assert!/assert_eq! macros"
    )]
    fn repo_predicate_name_lang_lowering_preserves_both_matchers() -> TestResult {
        let lowered = lower_sourcegraph_query_text(r#"repo:has.file(name:gate-a.rs, lang:rust)"#)
            .map_err(|err| -> Box<dyn std::error::Error> {
            format!("repo predicate lowering must succeed: {err:?}").into()
        })?;

        assert_eq!(
            lowered.expr,
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "repo.has.file".to_string(),
                args: vec![
                    LqPredicateArg::Filter {
                        name: "name".to_string(),
                        value: "gate-a.rs".to_string(),
                    },
                    LqPredicateArg::Filter {
                        name: "lang".to_string(),
                        value: "rust".to_string(),
                    },
                ],
            })
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts active predicate leaf lowering via assert!/assert_eq! macros"
    )]
    fn symbol_predicate_lowers_to_active_predicate_leaf() -> TestResult {
        let lowered = lower_sourcegraph_query_text(r#"symbol:has.name(MyTypeSymbol)"#).map_err(
            |err| -> Box<dyn std::error::Error> {
                format!("symbol predicate lowering must succeed: {err:?}").into()
            },
        )?;

        assert_eq!(
            lowered.expr,
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "symbol.has.name".to_string(),
                args: vec![LqPredicateArg::Keyword("MyTypeSymbol".to_string())],
            })
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts executable predicate normalization via assert!/assert_eq! macros"
    )]
    fn file_content_predicates_lower_to_executable_pattern_leaves() -> TestResult {
        let phrase = lower_sourcegraph_query_text(r#"file:contains("TODO")"#).map_err(
            |err| -> Box<dyn std::error::Error> {
                format!("file:contains lowering must succeed: {err:?}").into()
            },
        )?;
        assert_eq!(
            phrase.expr,
            LqExpr::Leaf(LqLeaf::Phrase("TODO".to_string()))
        );

        let regex = lower_sourcegraph_query_text(r"file:has.content(/TODO.*/)").map_err(
            |err| -> Box<dyn std::error::Error> {
                format!("file:has.content lowering must succeed: {err:?}").into()
            },
        )?;
        assert_eq!(
            regex.expr,
            LqExpr::Leaf(LqLeaf::Regex("TODO.*".to_string()))
        );

        let fallback = lower_sourcegraph_query_text("file:contains(path:src)").map_err(
            |err| -> Box<dyn std::error::Error> {
                format!("file:contains fallback lowering must succeed: {err:?}").into()
            },
        )?;
        assert_eq!(
            fallback.expr,
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "file.contains".to_string(),
                args: vec![LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src".to_string(),
                }],
            })
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts typed error code+message via assert!/assert_eq! macros"
    )]
    fn scoped_filters_under_or_and_not_fail_closed_with_typed_translate_errors() -> TestResult {
        let cases = vec![
            "repo:acme/demo needle OR fallback",
            "NOT repo:acme/demo needle",
        ];

        for raw in cases {
            let err = match lower_sourcegraph_query_text(raw) {
                Ok(query) => {
                    return Err(format!("expected fail-closed lowering, got {query:?}").into());
                }
                Err(err) => err,
            };
            let (code, message) = typed_error(err)?;
            assert_eq!(code, BridgeErrorCode::BridgeTranslateFail.as_code_str());
            assert_eq!(
                message,
                "bridge: scoped filters under OR/NOT are not representable on the active LQ wire"
            );
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts typed error code+message via assert!/assert_eq! macros"
    )]
    fn unsupported_filter_values_and_invalid_count_map_to_typed_errors() -> TestResult {
        for (raw, expected_code, expected_message) in [
            (
                "type:workspace invalid",
                BridgeErrorCode::BridgeUnsupportedFilter,
                "bridge: unsupported Sourcegraph type filter `workspace`",
            ),
            (
                "select:line invalid",
                BridgeErrorCode::BridgeUnsupportedFilter,
                "bridge: unsupported Sourcegraph select filter `line`",
            ),
            (
                "case:auto invalid",
                BridgeErrorCode::BridgeUnsupportedFilter,
                "bridge: unsupported Sourcegraph case filter `auto`",
            ),
            (
                "patterntype:fuzzy invalid",
                BridgeErrorCode::BridgeUnsupportedFilter,
                "bridge: unsupported patterntype `fuzzy`",
            ),
            (
                "fork:maybe invalid",
                BridgeErrorCode::BridgeUnsupportedDirective,
                "Sourcegraph `fork:` value must be one of yes|no|only",
            ),
            (
                "visibility:internal invalid",
                BridgeErrorCode::BridgeUnsupportedDirective,
                "Sourcegraph `visibility:` value must be one of public|private|any|include_forks|exclude_forks|only_forks|include_archived|exclude_archived|only_archived",
            ),
        ] {
            let err = match lower_sourcegraph_query_text(raw) {
                Ok(query) => {
                    return Err(format!("expected lowering failure, got {query:?}").into());
                }
                Err(err) => err,
            };
            let (code, message) = typed_error(err)?;
            assert_eq!(code, expected_code.as_code_str());
            assert_eq!(message, expected_message);
        }

        let err = match lower_sourcegraph_query_text("count:nan invalid") {
            Ok(query) => {
                return Err(format!("expected invalid count failure, got {query:?}").into());
            }
            Err(err) => err,
        };
        let (code, message) = typed_error(err)?;
        assert_eq!(code, BridgeErrorCode::BridgeTranslateFail.as_code_str());
        assert!(message.starts_with("bridge: invalid count `nan`:"));

        let err = match lower_sourcegraph_query_text("timeout:soon invalid") {
            Ok(query) => {
                return Err(format!("expected invalid timeout failure, got {query:?}").into());
            }
            Err(err) => err,
        };
        let (code, message) = typed_error(err)?;
        assert_eq!(code, BridgeErrorCode::BridgeTranslateFail.as_code_str());
        assert!(message.starts_with("bridge: invalid timeout `soon`:"));
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts parse failure code via assert!/assert_eq! macros"
    )]
    fn lq_syntax_fail_maps_to_parse_fail() -> TestResult {
        let err = match lower_lq_query_text("foo AND") {
            Ok(query) => {
                return Err(format!("expected parse failure, got {query:?}").into());
            }
            Err(err) => err,
        };
        let (code, message) = typed_error(err)?;
        assert_eq!(code, "PARSE_FAIL");
        assert!(!message.is_empty());
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts the frozen leaf-kind verdict matrix via assert_eq! macros"
    )]
    fn sourcegraph_structural_leaf_verdict_matrix_is_frozen() -> TestResult {
        // The leaf-kind legality matrix for the SG structural route. Flipping a
        // cell here is the only way to widen or narrow the mixed-domain subset,
        // and must travel with parity proof per the ADV-02 admission bar.
        assert_eq!(
            structural_leaf_verdict(&LqLeaf::Keyword("k".to_string())),
            StructuralLeafVerdict::PreserveLexical
        );
        assert_eq!(
            structural_leaf_verdict(&LqLeaf::RawString("r".to_string())),
            StructuralLeafVerdict::PreserveLexical,
            "ADV-02: RawString siblings are preserved to mirror native"
        );
        assert_eq!(
            structural_leaf_verdict(&LqLeaf::Phrase("p".to_string())),
            StructuralLeafVerdict::LowerPhraseBody("p")
        );
        assert_eq!(
            structural_leaf_verdict(&LqLeaf::Regex("x".to_string())),
            StructuralLeafVerdict::LowerRegexBody("x")
        );
        assert_eq!(
            structural_leaf_verdict(&LqLeaf::Predicate {
                name: "repo.has.file".to_string(),
                args: Vec::new(),
            }),
            StructuralLeafVerdict::PreserveLexical,
            "ADV-02: Predicate siblings are preserved; the lexical executor gates them as on native"
        );
        assert_eq!(
            structural_leaf_verdict(&LqLeaf::StructuralBlock(LqStructuralBlock {
                lang: None,
                nodes: Vec::new(),
                exprs: Vec::new(),
            })),
            StructuralLeafVerdict::TypedFail,
            "StructuralBlock is a pre-SG shape and must stay the only typed-fail leaf"
        );
        Ok(())
    }
}
