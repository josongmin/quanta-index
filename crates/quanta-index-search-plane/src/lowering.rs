use quanta_index_contract::{
    LqExpr, LqLeaf, LqPatternType, LqQuery, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_lq_bridge::{
    BridgeError, BridgeErrorCode, SourcegraphVersionTag, parse_sourcegraph, translate_query,
};
use quanta_index_lq_norm::{
    LqParseError, LqParseErrorCode, normalizer::normalize, parser::parse, tokenizer::tokenize,
};

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
    let version = SourcegraphVersionTag::supported().map_err(|err| map_bridge_error(&err))?;
    let query = translate_query(sourcegraph, &version, query_text.len())
        .map_err(|err| map_bridge_error(&err))?;
    match route {
        SourcegraphLoweringRoute::Lexical => reject_sourcegraph_structural_lexical_shape(query),
        SourcegraphLoweringRoute::Structural => {
            lower_sourcegraph_structural_shape(query_text, query)
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
    let structural_body = match &query.expr {
        LqExpr::Leaf(LqLeaf::Keyword(body) | LqLeaf::Phrase(body)) => body.clone(),
        LqExpr::Empty => {
            return Err(CoreError::Typed {
                code: BridgeErrorCode::BridgeTranslateFail
                    .as_code_str()
                    .to_string(),
                message:
                    "bridge: Sourcegraph structural route requires exactly one structural pattern body"
                        .to_string(),
            });
        }
        LqExpr::Leaf(LqLeaf::Regex(_)) => {
            return Err(CoreError::Typed {
                code: BridgeErrorCode::BridgeTranslateFail
                    .as_code_str()
                    .to_string(),
                message:
                    "bridge: regex pattern bodies are not supported on the Sourcegraph structural route"
                        .to_string(),
            });
        }
        LqExpr::Leaf(
            LqLeaf::RawString(_) | LqLeaf::StructuralBlock(_) | LqLeaf::Predicate { .. },
        )
        | LqExpr::Not(_)
        | LqExpr::All(_)
        | LqExpr::Any(_)
        | LqExpr::SemanticVector { .. } => {
            return Err(CoreError::Typed {
                code: BridgeErrorCode::BridgeTranslateFail
                    .as_code_str()
                    .to_string(),
                message:
                    "bridge: Sourcegraph structural route accepts exactly one keyword/quoted pattern body plus executable filters"
                        .to_string(),
            });
        }
    };
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
    query.expr = LqExpr::Leaf(LqLeaf::StructuralBlock(block));
    Ok(query)
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
        lower_lq_query_text, lower_sourcegraph_query_text, lower_sourcegraph_structural_query_text,
    };
    use quanta_index_contract::{
        LQ_VERSION_TAG, LqCase, LqCountBound, LqExpr, LqFileScope, LqFilter, LqLeaf, LqPatternType,
        LqPredicateArg, LqSelect, LqSpan, LqStructuralExpr, LqType, LqVisibility, LqYesNoOnly,
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
        let raw = "repo:acme/demo lang:rust case:yes count:25 patterntype:regexp select:content.match needle";
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
        assert_eq!(lowered.options.pattern_type, LqPatternType::Structural);
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
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts typed error code+message via assert!/assert_eq! macros"
    )]
    fn sourcegraph_structural_route_rejects_boolean_pattern_composition() -> TestResult {
        let err = match lower_sourcegraph_structural_query_text(
            r#"patterntype:structural "function_item" OR "identifier""#,
        ) {
            Ok(query) => {
                return Err(
                    format!("expected SG structural boolean rejection, got {query:?}").into(),
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
}
