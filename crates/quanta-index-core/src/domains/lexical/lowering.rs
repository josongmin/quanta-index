use quanta_index_contract::{
    LQ_VERSION_TAG, LqCase, LqCountBound, LqExpr, LqFileScope, LqFilter, LqLeaf, LqOptions,
    LqPatternType, LqPredicateArg, LqQuery, LqSelect, LqSpan, LqType, LqVisibility, LqYesNoOnly,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_lq_bridge::{
    BridgeError, BridgeErrorCode, LqDirective as BridgeDirective, SourcegraphVersionTag,
    parse_sourcegraph, translate,
};
use quanta_index_lq_norm::{
    LqParseError, LqParseErrorCode, normalizer::normalize, parser::parse, tokenizer::tokenize,
};

use crate::error::CoreError;

pub fn lower_lexical_text_query(
    request: &TextQueryRequest,
) -> Result<LqQuery, CoreError> {
    match request.syntax {
        TextQuerySyntax::Native => lower_lq_query_text(&request.query_text),
        TextQuerySyntax::Sourcegraph => lower_sourcegraph_query_text(&request.query_text),
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
        BridgeDirective::Predicate { name, args_raw } => {
            lower_bridge_predicate(name.as_ref(), args_raw.as_ref())
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

enum BridgePredicateArg {
    Phrase(String),
    RawString(String),
    Bare(String),
}

fn lower_bridge_predicate(name: &str, args_raw: &str) -> Result<LqExpr, CoreError> {
    let args = parse_bridge_predicate_args(args_raw)?;
    if let Some(executable) = lower_executable_bridge_predicate(name, &args) {
        return Ok(executable);
    }
    Ok(LqExpr::Leaf(LqLeaf::Predicate {
        name: name.to_string(),
        args: args.into_iter().map(to_lq_predicate_arg).collect(),
    }))
}

fn lower_executable_bridge_predicate(
    name: &str,
    args: &[BridgePredicateArg],
) -> Option<LqExpr> {
    match name {
        "file.contains" | "file.has.content" if args.len() == 1 => {
            args.first().and_then(lower_file_content_predicate_arg)
        }
        _ => None,
    }
}

fn lower_file_content_predicate_arg(arg: &BridgePredicateArg) -> Option<LqExpr> {
    match arg {
        BridgePredicateArg::Phrase(text) => Some(LqExpr::Leaf(LqLeaf::Phrase(text.clone()))),
        BridgePredicateArg::RawString(text) => {
            Some(LqExpr::Leaf(LqLeaf::RawString(text.clone())))
        }
        BridgePredicateArg::Bare(text) => {
            if text.split_once(':').is_some() {
                return None;
            }
            if let Some(regex) = strip_regex_delimiters(text) {
                return Some(LqExpr::Leaf(LqLeaf::Regex(regex.to_string())));
            }
            Some(LqExpr::Leaf(LqLeaf::Keyword(text.clone())))
        }
    }
}

fn strip_regex_delimiters(text: &str) -> Option<&str> {
    text.strip_prefix('/').and_then(|trimmed| trimmed.strip_suffix('/'))
}

fn to_lq_predicate_arg(arg: BridgePredicateArg) -> LqPredicateArg {
    match arg {
        BridgePredicateArg::Phrase(value) => LqPredicateArg::Phrase(value),
        BridgePredicateArg::RawString(value) => LqPredicateArg::RawString(value),
        BridgePredicateArg::Bare(value) => classify_bare_predicate_arg(value),
    }
}

fn classify_bare_predicate_arg(value: String) -> LqPredicateArg {
    if let Some((name, value)) = value.split_once(':') {
        return LqPredicateArg::Filter {
            name: name.to_string(),
            value: value.to_string(),
        };
    }
    if let Ok(number) = value.parse::<i64>() {
        return LqPredicateArg::Number(number);
    }
    LqPredicateArg::Keyword(value)
}

fn parse_bridge_predicate_args(raw: &str) -> Result<Vec<BridgePredicateArg>, CoreError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let bytes = raw.as_bytes();
    let mut pos: usize = 0;
    let mut out: Vec<BridgePredicateArg> = Vec::new();
    while pos < bytes.len() {
        while let Some(&b) = bytes.get(pos) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                pos = pos.saturating_add(1);
            } else {
                break;
            }
        }
        if pos >= bytes.len() {
            break;
        }

        let arg = match bytes.get(pos).copied() {
            Some(b'"') => {
                let (consumed, value) = read_quoted_bridge_arg(bytes, pos, b'"')?;
                pos = consumed;
                BridgePredicateArg::Phrase(value)
            }
            Some(b'\'') => {
                let (consumed, value) = read_quoted_bridge_arg(bytes, pos, b'\'')?;
                pos = consumed;
                BridgePredicateArg::RawString(value)
            }
            Some(_) => {
                let (consumed, value) = read_bare_bridge_arg(bytes, pos)?;
                pos = consumed;
                let value = value.trim();
                if value.is_empty() {
                    return Err(predicate_arg_error("bridge: empty predicate argument"));
                }
                BridgePredicateArg::Bare(value.to_string())
            }
            None => break,
        };
        out.push(arg);

        while let Some(&b) = bytes.get(pos) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                pos = pos.saturating_add(1);
            } else {
                break;
            }
        }
        if pos >= bytes.len() {
            break;
        }
        match bytes.get(pos).copied() {
            Some(b',') => {
                pos = pos.saturating_add(1);
            }
            Some(_) => {
                return Err(predicate_arg_error(
                    "bridge: unexpected character while parsing predicate arguments",
                ));
            }
            None => break,
        }
    }

    Ok(out)
}

fn read_quoted_bridge_arg(
    bytes: &[u8],
    start: usize,
    terminator: u8,
) -> Result<(usize, String), CoreError> {
    let mut pos = start.saturating_add(1);
    let mut buf = String::new();
    while let Some(&b) = bytes.get(pos) {
        if b == terminator {
            return Ok((pos.saturating_add(1), buf));
        }
        if b == b'\\' && terminator == b'"' {
            let after = pos.saturating_add(1);
            let Some(&esc) = bytes.get(after) else {
                return Err(predicate_arg_error(
                    "bridge: trailing backslash in predicate phrase argument",
                ));
            };
            let mapped = match esc {
                b'\\' => '\\',
                b'"' => '"',
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                _ => {
                    return Err(predicate_arg_error(
                        "bridge: unknown escape in predicate phrase argument",
                    ));
                }
            };
            buf.push(mapped);
            pos = after.saturating_add(1);
            continue;
        }
        let rest = bytes.get(pos..).unwrap_or(&[]);
        let ch = next_utf8_char(rest)?;
        buf.push(ch);
        pos = pos.saturating_add(ch.len_utf8());
    }
    Err(predicate_arg_error(
        "bridge: unterminated quoted predicate argument",
    ))
}

fn read_bare_bridge_arg(bytes: &[u8], start: usize) -> Result<(usize, String), CoreError> {
    let mut pos = start;
    let mut buf = String::new();
    while let Some(&b) = bytes.get(pos) {
        if b == b',' {
            break;
        }
        let rest = bytes.get(pos..).unwrap_or(&[]);
        let ch = next_utf8_char(rest)?;
        buf.push(ch);
        pos = pos.saturating_add(ch.len_utf8());
    }
    Ok((pos, buf))
}

fn next_utf8_char(bytes: &[u8]) -> Result<char, CoreError> {
    let text = core::str::from_utf8(bytes).map_err(|err| CoreError::Typed {
        code: BridgeErrorCode::BridgeTranslateFail
            .as_code_str()
            .to_string(),
        message: format!("bridge: invalid UTF-8 in predicate argument: {err}"),
    })?;
    text.chars()
        .next()
        .ok_or_else(|| predicate_arg_error("bridge: invalid UTF-8 in predicate argument"))
}

fn predicate_arg_error(message: &str) -> CoreError {
    CoreError::Typed {
        code: BridgeErrorCode::BridgeTranslateFail
            .as_code_str()
            .to_string(),
        message: message.to_string(),
    }
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
        "path" => query.filters.push(LqFilter::File {
            pattern: value.to_string(),
            scope: LqFileScope::PathOnly,
        }),
        "lang" => query.filters.push(LqFilter::Lang {
            id: value.to_string(),
        }),
        "author" => query.filters.push(LqFilter::Author {
            pattern: value.to_string(),
        }),
        "committer" => query.filters.push(LqFilter::Committer {
            pattern: value.to_string(),
        }),
        "message" => query.filters.push(LqFilter::Message {
            pattern: value.to_string(),
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
        "dirty" => query.filters.push(LqFilter::Dirty {
            mode: lower_yes_no_only_filter("dirty", value)?,
        }),
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
        "fork" => query.filters.push(LqFilter::Fork {
            mode: lower_yes_no_only_filter("fork", value)?,
        }),
        "archived" => query.filters.push(LqFilter::Archived {
            mode: lower_yes_no_only_filter("archived", value)?,
        }),
        "visibility" => query.filters.push(lower_visibility_filter(value)?),
        "context" => query.filters.push(LqFilter::Context {
            name: value.to_string(),
        }),
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

fn lower_yes_no_only_filter(name: &str, value: &str) -> Result<LqYesNoOnly, CoreError> {
    match value {
        "yes" => Ok(LqYesNoOnly::Yes),
        "no" => Ok(LqYesNoOnly::No),
        "only" => Ok(LqYesNoOnly::Only),
        other => Err(CoreError::Typed {
            code: BridgeErrorCode::BridgeUnsupportedFilter
                .as_code_str()
                .to_string(),
            message: format!("bridge: unsupported Sourcegraph {name} filter `{other}`"),
        }),
    }
}

fn lower_visibility_filter(value: &str) -> Result<LqFilter, CoreError> {
    match value {
        "public" => Ok(LqFilter::Visibility {
            mode: LqVisibility::Public,
        }),
        "private" => Ok(LqFilter::Visibility {
            mode: LqVisibility::Private,
        }),
        "any" => Ok(LqFilter::Visibility {
            mode: LqVisibility::Any,
        }),
        "include_forks" => Ok(LqFilter::Fork {
            mode: LqYesNoOnly::Yes,
        }),
        "exclude_forks" => Ok(LqFilter::Fork {
            mode: LqYesNoOnly::No,
        }),
        "only_forks" => Ok(LqFilter::Fork {
            mode: LqYesNoOnly::Only,
        }),
        "include_archived" => Ok(LqFilter::Archived {
            mode: LqYesNoOnly::Yes,
        }),
        "exclude_archived" => Ok(LqFilter::Archived {
            mode: LqYesNoOnly::No,
        }),
        "only_archived" => Ok(LqFilter::Archived {
            mode: LqYesNoOnly::Only,
        }),
        other => Err(CoreError::Typed {
            code: BridgeErrorCode::BridgeUnsupportedFilter
                .as_code_str()
                .to_string(),
            message: format!("bridge: unsupported Sourcegraph visibility filter `{other}`"),
        }),
    }
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
    use super::{
        bridge_directive_to_query, lower_lq_query_text, lower_sourcegraph_query_text,
    };
    use crate::error::CoreError;
    use quanta_index_contract::{
        LQ_VERSION_TAG, LqCase, LqCountBound, LqExpr, LqFileScope, LqFilter, LqLeaf,
        LqPatternType, LqPredicateArg, LqSelect, LqSpan, LqVisibility, LqYesNoOnly,
    };
    use quanta_index_lq_bridge::{BridgeErrorCode, LqDirective as BridgeDirective};

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

    fn predicate(name: &str, args_raw: &str) -> BridgeDirective {
        BridgeDirective::Predicate {
            name: name.into(),
            args_raw: args_raw.into(),
        }
    }

    fn typed_error(err: CoreError) -> (String, String) {
        match err {
            CoreError::Typed { code, message } => (code, message),
            other => panic!("expected typed error, got {other:?}"),
        }
    }

    #[test]
    fn nested_boolean_lowering_preserves_executable_pattern_structure() {
        let query = bridge_directive_to_query(
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
        .expect("nested boolean lowering should succeed");

        assert_eq!(query.lq_version, LQ_VERSION_TAG);
        assert_eq!(
            query.expr,
            LqExpr::All(vec![
                LqExpr::Leaf(LqLeaf::Keyword("alpha".to_string())),
                LqExpr::Any(vec![
                    LqExpr::Leaf(LqLeaf::Phrase("beta gamma".to_string())),
                    LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Regex("c.*d".to_string())))),
                ]),
                LqExpr::Leaf(LqLeaf::Keyword("omega".to_string())),
            ])
        );
        assert!(query.filters.is_empty());
        assert_eq!(query.options.pattern_type, LqPatternType::Standard);
    }

    #[test]
    fn filtered_sourcegraph_body_populates_typed_filters_and_options() {
        let raw =
            "repo:acme/demo lang:rust case:yes count:25 patterntype:regexp select:content.match needle";
        let query =
            lower_sourcegraph_query_text(raw).expect("sourcegraph lowering should succeed");

        assert_eq!(query.lq_version, LQ_VERSION_TAG);
        assert_eq!(query.expr, LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())));
        assert_eq!(
            query.filters,
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
        assert_eq!(query.options.case, Some(LqCase::Sensitive));
        assert_eq!(query.options.count, Some(LqCountBound::Bounded(25)));
        assert_eq!(query.options.pattern_type, LqPatternType::Regexp);
        assert_eq!(
            query.source_span,
            LqSpan::eof(u32::try_from(raw.len()).expect("raw query len should fit in u32"))
        );
    }

    #[test]
    fn richer_bridge_filters_lower_into_active_lq_contract() {
        let query = bridge_directive_to_query(
            BridgeDirective::Filtered {
                filters: vec![
                    filter("path", "src/lib.rs"),
                    filter("fork", "only"),
                    filter("archived", "no"),
                    filter("visibility", "private"),
                    filter("context", "team-search"),
                ],
                body: Box::new(pattern("literal", "needle")),
            },
            "path:src/lib.rs needle",
        )
        .expect("richer bridge filter lowering should succeed");

        assert_eq!(query.expr, LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())));
        assert_eq!(
            query.filters,
            vec![
                LqFilter::File {
                    pattern: "src/lib.rs".to_string(),
                    scope: LqFileScope::PathOnly,
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
    }

    #[test]
    fn normalized_visibility_bridge_payloads_map_to_fork_and_archived_filters() {
        let query = bridge_directive_to_query(
            BridgeDirective::Filtered {
                filters: vec![
                    filter("visibility", "include_forks"),
                    filter("visibility", "only_archived"),
                ],
                body: Box::new(pattern("literal", "needle")),
            },
            "fork:yes archived:only needle",
        )
        .expect("normalized visibility lowering should succeed");

        assert_eq!(
            query.filters,
            vec![
                LqFilter::Fork {
                    mode: LqYesNoOnly::Yes,
                },
                LqFilter::Archived {
                    mode: LqYesNoOnly::Only,
                },
            ]
        );
    }

    #[test]
    fn repo_predicates_lower_to_active_predicate_leaves() {
        let query = bridge_directive_to_query(
            predicate("repo.has.file", r#"path:src/lib.rs, name:"Cargo.toml""#),
            "repo:has.file(path:src/lib.rs, name:\"Cargo.toml\")",
        )
        .expect("repo predicate lowering should succeed");

        assert_eq!(
            query.expr,
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
    }

    #[test]
    fn file_content_predicates_lower_to_executable_pattern_leaves() {
        let phrase = lower_sourcegraph_query_text(r#"file:contains("TODO")"#)
            .expect("file:contains lowering should succeed");
        assert_eq!(phrase.expr, LqExpr::Leaf(LqLeaf::Phrase("TODO".to_string())));

        let regex = lower_sourcegraph_query_text(r"file:has.content(/TODO.*/)")
            .expect("file:has.content lowering should succeed");
        assert_eq!(
            regex.expr,
            LqExpr::Leaf(LqLeaf::Regex("TODO.*".to_string()))
        );

        let fallback = lower_sourcegraph_query_text("file:contains(path:src)")
            .expect("file:contains fallback lowering should succeed");
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
    }

    #[test]
    fn scoped_filters_under_or_and_not_fail_closed_with_typed_translate_errors() {
        let scoped = BridgeDirective::Filtered {
            filters: vec![filter("repo", "acme/demo")],
            body: Box::new(pattern("literal", "needle")),
        };
        let cases = vec![
            BridgeDirective::Or(vec![scoped.clone(), pattern("literal", "fallback")]),
            BridgeDirective::Not(Box::new(scoped)),
        ];

        for directive in cases {
            let (code, message) = typed_error(
                bridge_directive_to_query(directive, "repo:acme/demo needle")
                    .expect_err("scoped filters under OR/NOT must fail closed"),
            );
            assert_eq!(code, BridgeErrorCode::BridgeTranslateFail.as_code_str());
            assert_eq!(
                message,
                "bridge: scoped filters under OR/NOT are not representable on the active LQ wire"
            );
        }
    }

    #[test]
    fn unsupported_filter_values_and_invalid_count_map_to_typed_errors() {
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
                "fork",
                "maybe",
                "bridge: unsupported Sourcegraph fork filter `maybe`",
            ),
            (
                "visibility",
                "internal",
                "bridge: unsupported Sourcegraph visibility filter `internal`",
            ),
        ] {
            let (code, message) = typed_error(
                bridge_directive_to_query(filter(name, value), "invalid")
                    .expect_err("unsupported filter value must fail"),
            );
            assert_eq!(code, BridgeErrorCode::BridgeUnsupportedFilter.as_code_str());
            assert_eq!(message, expected_message);
        }

        let (code, message) = typed_error(
            bridge_directive_to_query(filter("count", "nan"), "invalid")
                .expect_err("invalid count must fail"),
        );
        assert_eq!(code, BridgeErrorCode::BridgeUnsupportedFilter.as_code_str());
        assert!(message.starts_with("bridge: invalid count `nan`:"));
    }

    #[test]
    fn lq_syntax_fail_maps_to_parse_fail() {
        let (code, message) =
            typed_error(lower_lq_query_text("foo AND").expect_err("invalid LQ should fail"));

        assert_eq!(code, "PARSE_FAIL");
        assert!(message.contains("SYNTAX_ERROR"));
        assert!(message.contains("expected term after boolean operator"));
    }
}
