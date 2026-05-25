use quanta_index_contract::{
    LQ_VERSION_TAG, LqCase, LqCountBound, LqExpr, LqFileScope, LqFilter, LqLeaf, LqOptions,
    LqPatternType, LqQuery, LqSelect, LqSpan, LqType, SearchPlaneLexicalTextQueryRequestV2,
    SearchQuerySyntaxV1,
};
use quanta_index_lq_bridge::{
    BridgeError, BridgeErrorCode, LqDirective as BridgeDirective, SourcegraphVersionTag,
    parse_sourcegraph, translate,
};
use quanta_index_lq_norm::{
    LqParseError, LqParseErrorCode, normalizer::normalize, parser::parse, tokenizer::tokenize,
};

use quanta_index_core::CoreError;

pub fn lower_lexical_text_query(
    request: &SearchPlaneLexicalTextQueryRequestV2,
) -> Result<LqQuery, CoreError> {
    match request.syntax {
        SearchQuerySyntaxV1::Lq => lower_lq_query_text(&request.query_text),
        SearchQuerySyntaxV1::Sourcegraph => lower_sourcegraph_query_text(&request.query_text),
    }
}

pub fn lower_sourcegraph_query_text(query_text: &str) -> Result<LqQuery, CoreError> {
    let sourcegraph = parse_sourcegraph(query_text).map_err(|err| map_bridge_error(&err))?;
    let version = SourcegraphVersionTag::supported().map_err(|err| map_bridge_error(&err))?;
    let lowered = translate(sourcegraph, &version).map_err(|err| map_bridge_error(&err))?;
    bridge_directive_to_query(lowered, query_text)
}

fn lower_lq_query_text(query_text: &str) -> Result<LqQuery, CoreError> {
    let tokens = tokenize(query_text).map_err(|err| map_lq_error(&err))?;
    let parsed = parse(&tokens, query_text).map_err(|err| map_lq_error(&err))?;
    normalize(parsed).map_err(|err| map_lq_error(&err))
}

fn bridge_directive_to_query(
    directive: BridgeDirective,
    query_text: &str,
) -> Result<LqQuery, CoreError> {
    let mut query = LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Empty,
        filters: Vec::new(),
        directives: Vec::new(),
        options: LqOptions::defaults(),
        source_span: LqSpan::eof(u32::try_from(query_text.len()).map_or(u32::MAX, |n| n)),
    };
    query.expr = lower_bridge_expr(directive, &mut query, true)?;
    Ok(query)
}

fn lower_bridge_expr(
    directive: BridgeDirective,
    query: &mut LqQuery,
    allow_scoped_filters: bool,
) -> Result<LqExpr, CoreError> {
    match directive {
        BridgeDirective::Pattern { kind, body } => {
            lower_bridge_pattern(kind.as_ref(), body.as_ref())
        }
        BridgeDirective::Filter { name, value } => {
            if !allow_scoped_filters {
                return Err(CoreError::Typed {
                    code: BridgeErrorCode::BridgeTranslateFail
                        .as_code_str()
                        .to_string(),
                    message: format!(
                        "bridge: scoped filter `{name}` cannot be lowered into the active LQ contract"
                    ),
                });
            }
            apply_bridge_filter(name.as_ref(), value.as_ref(), query)?;
            Ok(LqExpr::Empty)
        }
        BridgeDirective::And(items) => {
            let mut out: Vec<LqExpr> = Vec::new();
            for item in items {
                let lowered = lower_bridge_expr(item, query, allow_scoped_filters)?;
                if !matches!(lowered, LqExpr::Empty) {
                    out.push(lowered);
                }
            }
            Ok(collapse_exprs(out, true))
        }
        BridgeDirective::Or(items) => {
            let mut out: Vec<LqExpr> = Vec::new();
            for item in items {
                let lowered = lower_bridge_expr(item, query, false)?;
                if matches!(lowered, LqExpr::Empty) {
                    return Err(CoreError::Typed {
                        code: BridgeErrorCode::BridgeTranslateFail.as_code_str().to_string(),
                        message: "bridge: OR branch lowered to filters-only query, which the active LQ contract cannot represent".to_string(),
                    });
                }
                out.push(lowered);
            }
            Ok(collapse_exprs(out, false))
        }
        BridgeDirective::Not(inner) => {
            let lowered = lower_bridge_expr(*inner, query, false)?;
            if matches!(lowered, LqExpr::Empty) {
                return Err(CoreError::Typed {
                    code: BridgeErrorCode::BridgeTranslateFail.as_code_str().to_string(),
                    message: "bridge: NOT over filters-only subtree cannot be lowered into the active LQ contract".to_string(),
                });
            }
            Ok(LqExpr::Not(Box::new(lowered)))
        }
        BridgeDirective::Filtered { filters, body } => {
            if !allow_scoped_filters {
                return Err(CoreError::Typed {
                    code: BridgeErrorCode::BridgeTranslateFail.as_code_str().to_string(),
                    message: "bridge: scoped filters under OR/NOT are not representable on the active LQ wire".to_string(),
                });
            }
            for filter in filters {
                let lowered = lower_bridge_expr(filter, query, true)?;
                if !matches!(lowered, LqExpr::Empty) {
                    return Err(CoreError::Typed {
                        code: BridgeErrorCode::BridgeTranslateFail.as_code_str().to_string(),
                        message: "bridge: expected filter-only bridge node while lowering filtered subtree".to_string(),
                    });
                }
            }
            lower_bridge_expr(*body, query, true)
        }
    }
}

fn collapse_exprs(items: Vec<LqExpr>, all: bool) -> LqExpr {
    match items.len() {
        0 => LqExpr::Empty,
        1 => items.into_iter().next().map_or(LqExpr::Empty, |item| item),
        _ if all => LqExpr::All(items),
        _ => LqExpr::Any(items),
    }
}

fn lower_bridge_pattern(kind: &str, body: &str) -> Result<LqExpr, CoreError> {
    let leaf = match kind {
        "literal" | "keyword" => LqLeaf::Keyword(body.to_string()),
        "phrase" => LqLeaf::Phrase(body.to_string()),
        "regex" => LqLeaf::Regex(body.to_string()),
        other => {
            return Err(CoreError::Typed {
                code: BridgeErrorCode::BridgeTranslateFail
                    .as_code_str()
                    .to_string(),
                message: format!("bridge: unsupported lowered pattern kind `{other}`"),
            });
        }
    };
    Ok(LqExpr::Leaf(leaf))
}

fn apply_bridge_filter(name: &str, value: &str, query: &mut LqQuery) -> Result<(), CoreError> {
    match name {
        "repo" => query.filters.push(LqFilter::Repo {
            pattern: value.to_string(),
            revs: Vec::new(),
        }),
        "file" => query.filters.push(LqFilter::File {
            pattern: value.to_string(),
            scope: LqFileScope::NameAndPath,
        }),
        "lang" => query.filters.push(LqFilter::Lang {
            id: value.to_string(),
        }),
        "type" => {
            let kind = match value {
                "file" => LqType::File,
                "path" => LqType::Path,
                "symbol" => LqType::Symbol,
                "commit" => LqType::Commit,
                "diff" => LqType::Diff,
                "repo" => LqType::Repo,
                other => {
                    return Err(CoreError::Typed {
                        code: BridgeErrorCode::BridgeUnsupportedFilter
                            .as_code_str()
                            .to_string(),
                        message: format!("bridge: unsupported Sourcegraph type filter `{other}`"),
                    });
                }
            };
            query.filters.push(LqFilter::Type { kind });
        }
        "select" => {
            let dim = match value {
                "repo" => LqSelect::Repo,
                "file" => LqSelect::File,
                "path" => LqSelect::Path,
                "symbol" => LqSelect::Symbol,
                "content" => LqSelect::Content,
                "content.match" => LqSelect::ContentMatch,
                other => {
                    return Err(CoreError::Typed {
                        code: BridgeErrorCode::BridgeUnsupportedFilter
                            .as_code_str()
                            .to_string(),
                        message: format!("bridge: unsupported Sourcegraph select filter `{other}`"),
                    });
                }
            };
            query.filters.push(LqFilter::Select { dim });
        }
        "case" => {
            query.options.case = Some(match value {
                "yes" => LqCase::Sensitive,
                "no" => LqCase::Insensitive,
                other => {
                    return Err(CoreError::Typed {
                        code: BridgeErrorCode::BridgeUnsupportedFilter
                            .as_code_str()
                            .to_string(),
                        message: format!("bridge: unsupported Sourcegraph case filter `{other}`"),
                    });
                }
            });
        }
        "count" => {
            query.options.count = Some(if value == "all" {
                LqCountBound::All
            } else {
                let parsed = value.parse::<u32>().map_err(|err| CoreError::Typed {
                    code: BridgeErrorCode::BridgeUnsupportedFilter
                        .as_code_str()
                        .to_string(),
                    message: format!("bridge: invalid count `{value}`: {err}"),
                })?;
                LqCountBound::Bounded(parsed)
            });
        }
        "patterntype" => {
            query.options.pattern_type = match value {
                "literal" => LqPatternType::Literal,
                "keyword" => LqPatternType::Keyword,
                "standard" => LqPatternType::Standard,
                "regexp" => LqPatternType::Regexp,
                "structural" => LqPatternType::Structural,
                other => {
                    return Err(CoreError::Typed {
                        code: BridgeErrorCode::BridgeUnsupportedFilter
                            .as_code_str()
                            .to_string(),
                        message: format!("bridge: unsupported patterntype `{other}`"),
                    });
                }
            };
        }
        "visibility" => {
            return Err(CoreError::Typed {
                code: BridgeErrorCode::BridgeUnsupportedFilter
                    .as_code_str()
                    .to_string(),
                message: format!(
                    "bridge: visibility filter `{value}` is not yet representable on the active LQ contract"
                ),
            });
        }
        other => {
            return Err(CoreError::Typed {
                code: BridgeErrorCode::BridgeUnsupportedFilter
                    .as_code_str()
                    .to_string(),
                message: format!("bridge: unsupported filter `{other}`"),
            });
        }
    }
    Ok(())
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
        message: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{bridge_directive_to_query, lower_lq_query_text, lower_sourcegraph_query_text};
    use quanta_index_contract::{
        LQ_VERSION_TAG, LqCase, LqCountBound, LqExpr, LqFilter, LqLeaf, LqPatternType, LqSelect,
        LqSpan,
    };
    use quanta_index_core::CoreError;
    use quanta_index_lq_bridge::{BridgeErrorCode, LqDirective as BridgeDirective};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn pattern(kind: &str, body: &str) -> BridgeDirective {
        BridgeDirective::Pattern {
            kind: kind.into(),
            body: body.into(),
        }
    }

    fn filter(name: &str, value: &str) -> BridgeDirective {
        BridgeDirective::Filter {
            name: name.into(),
            value: value.into(),
        }
    }

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
        let lowered = bridge_directive_to_query(
            BridgeDirective::And(vec![
                pattern("literal", "alpha"),
                BridgeDirective::Or(vec![
                    pattern("phrase", "beta gamma"),
                    BridgeDirective::Not(Box::new(pattern("regex", "c.*d"))),
                ]),
                pattern("keyword", "omega"),
            ]),
            "alpha OR beta gamma OR c.*d",
        )
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
    fn scoped_filters_under_or_and_not_fail_closed_with_typed_translate_errors() -> TestResult {
        let scoped = BridgeDirective::Filtered {
            filters: vec![filter("repo", "acme/demo")],
            body: Box::new(pattern("literal", "needle")),
        };
        let cases = vec![
            BridgeDirective::Or(vec![scoped.clone(), pattern("literal", "fallback")]),
            BridgeDirective::Not(Box::new(scoped)),
        ];

        for directive in cases {
            let err = match bridge_directive_to_query(directive, "repo:acme/demo needle") {
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
        for (name, value, expected_message) in [
            (
                "type",
                "workspace",
                "bridge: unsupported Sourcegraph type filter `workspace`",
            ),
            (
                "select",
                "line",
                "bridge: unsupported Sourcegraph select filter `line`",
            ),
            (
                "case",
                "auto",
                "bridge: unsupported Sourcegraph case filter `auto`",
            ),
            (
                "patterntype",
                "fuzzy",
                "bridge: unsupported patterntype `fuzzy`",
            ),
            (
                "visibility",
                "private",
                "bridge: visibility filter `private` is not yet representable on the active LQ contract",
            ),
        ] {
            let err = match bridge_directive_to_query(filter(name, value), "invalid") {
                Ok(query) => {
                    return Err(format!("expected lowering failure, got {query:?}").into());
                }
                Err(err) => err,
            };
            let (code, message) = typed_error(err)?;
            assert_eq!(code, BridgeErrorCode::BridgeUnsupportedFilter.as_code_str());
            assert_eq!(message, expected_message);
        }

        let err = match bridge_directive_to_query(filter("count", "nan"), "invalid") {
            Ok(query) => {
                return Err(format!("expected invalid count failure, got {query:?}").into());
            }
            Err(err) => err,
        };
        let (code, message) = typed_error(err)?;
        assert_eq!(code, BridgeErrorCode::BridgeUnsupportedFilter.as_code_str());
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
