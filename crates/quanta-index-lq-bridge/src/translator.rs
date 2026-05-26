//! Sourcegraph → LQ lowering.
//!
//! [`translate`] is the one-way translator surface from
//! [BRIDGE-01](../../../../docs/plans/may-24-lexical-indexing-sorucegraph/tickets/BRIDGE-01.md).
//! It lowers an already-parsed [`SgQuery`] directly into the canonical
//! `LqQuery` wire shape. A small internal placeholder tree still exists only
//! as a local implementation detail so the bridge can preserve the existing
//! adopted/normalized/refused decision table without keeping a second public
//! compiler stage in `search-plane`.
//!
//! ## Decision table (from
//! [BRIDGE-01 § 5 step 5 / § 6.1](../../../../docs/plans/may-24-lexical-indexing-sorucegraph/tickets/BRIDGE-01.md))
//!
//! | Sourcegraph filter | Bucket | LQ lowering |
//! |---|---|---|
//! | `repo:` / `file:` / `path:` / `lang:` / `rev:` / `author:` / `committer:` / `message:` / `case:` / `select:` / `count:` / `type:` / `patterntype:` | adopted | 1:1 `LqFilter` |
//! | `dirty:` / `fork:` / `archived:` / `visibility:` / `context:` | adopted | active LQ filter surface |
//! | `content:` | normalized | `Pattern{kind: Literal, body: <value>}` |
//! | `index:` / `boost:` / `timeout:` | refused | `BRIDGE_UNSUPPORTED_DIRECTIVE` |
//! | `file:contains(...)` / `file:has.content(...)` | adopted | active LQ predicate leaf or executable pattern lowering downstream |
//! | remaining `repo:` / `file:` predicates | adopted | active LQ predicate leaf lowering downstream |
//! | unknown name | refused | `BRIDGE_UNSUPPORTED_FILTER` |
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use crate::errors::{BridgeError, BridgeErrorCode};
use crate::syntax::{SgFilter, SgQuery};
use crate::version::SourcegraphVersionTag;
use quanta_index_contract::{
    LQ_VERSION_TAG, LqCase, LqCountBound, LqExpr, LqFileScope, LqFilter, LqLeaf, LqOptions,
    LqPatternType, LqPredicateArg, LqQuery, LqSelect, LqSpan, LqType, LqVisibility, LqYesNoOnly,
};

/// Placeholder lowered LQ directive tree.
///
/// Shape mirrors the LQ canonical AST family closely enough that the
/// integration ticket can replace this type with the real `LqExpr`
/// via a mechanical mapping without further translator changes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum LqDirective {
    /// A lexical pattern (literal / phrase / regex). The `kind` field
    /// is preserved from the Sourcegraph parser, except translated
    /// `content:` values which lower as `literal`.
    Pattern {
        kind: Box<str>,
        body: Box<str>,
    },
    /// A `name:value` LQ filter, where `name` is the canonical LQ
    /// filter name after Sourcegraph normalization.
    Filter {
        name: Box<str>,
        value: Box<str>,
    },
    /// A predicate leaf placeholder. `name` is the canonical dot-joined
    /// active LQ predicate name (for example `repo.has.file`), while
    /// `args_raw` preserves the comma-separated predicate argument body for
    /// downstream typed lowering onto the active LQ predicate leaf.
    Predicate {
        name: Box<str>,
        args_raw: Box<str>,
    },
    And(Vec<LqDirective>),
    Or(Vec<LqDirective>),
    Not(Box<LqDirective>),
    /// A `Filtered` directive — one or more filters scoping a body
    /// subexpression. Matches LQ's canonical filter-prefix shape.
    Filtered {
        filters: Vec<LqDirective>,
        body: Box<LqDirective>,
    },
}

/// Translate a parsed Sourcegraph query into the internal bridge placeholder
/// tree.
fn translate_placeholder(
    sg: SgQuery,
    sg_version: &SourcegraphVersionTag,
) -> Result<LqDirective, BridgeError> {
    // The version tag is currently consumed only for refusal payloads
    // (none yet record it explicitly); referencing it here keeps the
    // parameter live for future use without a stale-arg warning.
    let _: &SourcegraphVersionTag = sg_version;
    translate_inner(sg)
}

/// Translate a parsed Sourcegraph query directly into canonical `LqQuery`.
pub fn translate_query(
    sg: SgQuery,
    sg_version: &SourcegraphVersionTag,
    source_len: usize,
) -> Result<LqQuery, BridgeError> {
    let directive = translate_placeholder(sg, sg_version)?;
    bridge_directive_to_query(directive, source_len)
}

fn translate_inner(sg: SgQuery) -> Result<LqDirective, BridgeError> {
    match sg {
        SgQuery::Pattern { kind, body } => Ok(LqDirective::Pattern {
            kind: Box::<str>::from(kind.as_str()),
            body,
        }),
        SgQuery::Predicate {
            scope,
            name,
            args_raw,
        } => Ok(LqDirective::Predicate {
            name: format!("{scope}.{name}").into_boxed_str(),
            args_raw,
        }),
        SgQuery::And(xs) => {
            let mut out: Vec<LqDirective> = Vec::with_capacity(xs.len());
            for x in xs {
                out.push(translate_inner(x)?);
            }
            Ok(LqDirective::And(out))
        }
        SgQuery::Or(xs) => {
            let mut out: Vec<LqDirective> = Vec::with_capacity(xs.len());
            for x in xs {
                out.push(translate_inner(x)?);
            }
            Ok(hoist_common_filtered_or(out))
        }
        SgQuery::Not(inner) => {
            let lowered = translate_inner(*inner)?;
            Ok(LqDirective::Not(Box::new(lowered)))
        }
        SgQuery::Filtered { filters, body } => translate_filtered(filters, *body),
    }
}

fn bridge_directive_to_query(
    directive: LqDirective,
    source_len: usize,
) -> Result<LqQuery, BridgeError> {
    let source_len = u32::try_from(source_len).map_err(|err| {
        BridgeError::translate_fail(format!("bridge: query length does not fit u32: {err}"))
    })?;
    let mut query = LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Empty,
        filters: Vec::new(),
        directives: Vec::new(),
        options: LqOptions::defaults(),
        source_span: LqSpan::eof(source_len),
    };
    query.expr = lower_bridge_expr(directive, &mut query, true)?;
    Ok(query)
}

fn translate_filtered(filters: Vec<SgFilter>, body: SgQuery) -> Result<LqDirective, BridgeError> {
    let mut lowered_filters: Vec<LqDirective> = Vec::with_capacity(filters.len());
    let mut content_patterns: Vec<LqDirective> = Vec::new();
    for f in filters {
        match lower_filter(&f)? {
            LowerOutcome::Filter(d) => lowered_filters.push(d),
            LowerOutcome::ContentPattern(d) => content_patterns.push(d),
            LowerOutcome::Drop => {}
        }
    }
    let body_lowered = translate_inner(body)?;
    // Build body: original body AND content-derived patterns, in
    // source order. If body is the empty placeholder Pattern{literal,
    // ""}, drop it.
    let mut body_terms: Vec<LqDirective> = Vec::new();
    if !is_empty_placeholder(&body_lowered) {
        body_terms.push(body_lowered);
    }
    for p in content_patterns {
        body_terms.push(p);
    }
    let body = match body_terms.len() {
        0 => LqDirective::Pattern {
            kind: Box::<str>::from("literal"),
            body: Box::<str>::from(""),
        },
        1 => match body_terms.into_iter().next() {
            Some(t) => t,
            None => {
                return Err(BridgeError::translate_fail(
                    "internal: empty body_terms after singleton check",
                ));
            }
        },
        _ => LqDirective::And(body_terms),
    };
    if lowered_filters.is_empty() {
        Ok(body)
    } else {
        Ok(LqDirective::Filtered {
            filters: lowered_filters,
            body: Box::new(body),
        })
    }
}

fn hoist_common_filtered_or(items: Vec<LqDirective>) -> LqDirective {
    let hoisted_filters = {
        let mut iter = items.iter();
        let Some(LqDirective::Filtered { filters, .. }) = iter.next() else {
            return LqDirective::Or(items);
        };
        if !iter.all(|item| {
            matches!(item, LqDirective::Filtered { filters: branch, .. } if branch == filters)
        }) {
            return LqDirective::Or(items);
        }
        filters.clone()
    };
    let mut bodies: Vec<LqDirective> = Vec::with_capacity(items.len());
    for item in items {
        let LqDirective::Filtered { body, .. } = item else {
            return LqDirective::Or(bodies);
        };
        bodies.push(*body);
    }
    LqDirective::Filtered {
        filters: hoisted_filters,
        body: Box::new(LqDirective::Or(bodies)),
    }
}

enum LowerOutcome {
    Filter(LqDirective),
    ContentPattern(LqDirective),
    Drop,
}

fn lower_filter(f: &SgFilter) -> Result<LowerOutcome, BridgeError> {
    match f {
        // Adopted 1:1.
        SgFilter::Repo(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("repo"),
            value: v.clone(),
        })),
        SgFilter::File(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("file"),
            value: v.clone(),
        })),
        SgFilter::Path(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("path"),
            value: v.clone(),
        })),
        SgFilter::Lang(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("lang"),
            value: v.clone(),
        })),
        SgFilter::Rev(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("rev"),
            value: v.clone(),
        })),
        SgFilter::Author(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("author"),
            value: v.clone(),
        })),
        SgFilter::Committer(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("committer"),
            value: v.clone(),
        })),
        SgFilter::Message(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("message"),
            value: v.clone(),
        })),
        SgFilter::Type(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("type"),
            value: v.clone(),
        })),
        SgFilter::Case(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("case"),
            value: v.clone(),
        })),
        SgFilter::Select(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("select"),
            value: v.clone(),
        })),
        SgFilter::Count(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("count"),
            value: v.clone(),
        })),
        SgFilter::Patterntype(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("patterntype"),
            value: v.clone(),
        })),
        SgFilter::Dirty(v) => validate_yes_no_only("dirty", v).map(|value| {
            LowerOutcome::Filter(LqDirective::Filter {
                name: Box::<str>::from("dirty"),
                value,
            })
        }),
        SgFilter::Fork(v) => validate_yes_no_only("fork", v).map(|value| {
            LowerOutcome::Filter(LqDirective::Filter {
                name: Box::<str>::from("fork"),
                value,
            })
        }),
        SgFilter::Archived(v) => validate_yes_no_only("archived", v).map(|value| {
            LowerOutcome::Filter(LqDirective::Filter {
                name: Box::<str>::from("archived"),
                value,
            })
        }),
        SgFilter::Visibility(v) => validate_visibility(v).map(|value| {
            LowerOutcome::Filter(LqDirective::Filter {
                name: Box::<str>::from("visibility"),
                value,
            })
        }),
        // Normalized: `content:` becomes a pattern leaf attached to
        // the body.
        SgFilter::Content(v) => Ok(LowerOutcome::ContentPattern(LqDirective::Pattern {
            kind: Box::<str>::from("literal"),
            body: v.clone(),
        })),
        SgFilter::Index(v) => match v.as_ref() {
            "yes" | "only" => Ok(LowerOutcome::Drop),
            "no" => Err(BridgeError::unsupported_directive(
                "index:no",
                "Sourcegraph `index:no` is refused because this stack is index-only",
            )),
            other => Err(BridgeError::unsupported_directive(
                &format!("index:{other}"),
                "Sourcegraph `index:` value must be one of yes|no|only",
            )),
        },
        SgFilter::Boost(v) => Err(BridgeError::unsupported_directive(
            &format!("boost:{v}"),
            "Sourcegraph `boost:` is not representable on the active LQ contract",
        )),
        SgFilter::Context(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("context"),
            value: v.clone(),
        })),
        SgFilter::Timeout(v) => Err(BridgeError::unsupported_directive(
            &format!("timeout:{v}"),
            "Sourcegraph `timeout:` is not representable on the active LQ contract",
        )),
    }
}

fn validate_yes_no_only(name: &str, value: &str) -> Result<Box<str>, BridgeError> {
    match value {
        "yes" | "no" | "only" => Ok(Box::<str>::from(value)),
        other => Err(BridgeError::unsupported_directive(
            &format!("{name}:{other}"),
            format!("Sourcegraph `{name}:` value must be one of yes|no|only"),
        )),
    }
}

fn validate_visibility(value: &str) -> Result<Box<str>, BridgeError> {
    match value {
        "public" | "private" | "any" | "include_forks" | "exclude_forks" | "only_forks"
        | "include_archived" | "exclude_archived" | "only_archived" => Ok(Box::<str>::from(value)),
        other => Err(BridgeError::unsupported_directive(
            &format!("visibility:{other}"),
            "Sourcegraph `visibility:` value must be one of public|private|any|include_forks|exclude_forks|only_forks|include_archived|exclude_archived|only_archived",
        )),
    }
}

fn is_empty_placeholder(d: &LqDirective) -> bool {
    matches!(d, LqDirective::Pattern { kind, body } if kind.as_ref() == "literal" && body.is_empty())
}

fn lower_bridge_expr(
    directive: LqDirective,
    query: &mut LqQuery,
    allow_scoped_filters: bool,
) -> Result<LqExpr, BridgeError> {
    match directive {
        LqDirective::Pattern { kind, body } => lower_bridge_pattern(kind.as_ref(), body.as_ref()),
        LqDirective::Filter { name, value } => {
            if !allow_scoped_filters {
                return Err(BridgeError::translate_fail(format!(
                    "bridge: scoped filter `{name}` cannot be lowered into the active LQ contract"
                )));
            }
            apply_bridge_filter(name.as_ref(), value.as_ref(), query)?;
            Ok(LqExpr::Empty)
        }
        LqDirective::Predicate { name, args_raw } => {
            lower_bridge_predicate(name.as_ref(), args_raw.as_ref())
        }
        LqDirective::And(items) => {
            let mut out: Vec<LqExpr> = Vec::new();
            for item in items {
                let lowered = lower_bridge_expr(item, query, allow_scoped_filters)?;
                if !matches!(lowered, LqExpr::Empty) {
                    out.push(lowered);
                }
            }
            Ok(collapse_exprs(out, true))
        }
        LqDirective::Or(items) => {
            let mut out: Vec<LqExpr> = Vec::new();
            for item in items {
                let lowered = lower_bridge_expr(item, query, false)?;
                if matches!(lowered, LqExpr::Empty) {
                    return Err(BridgeError::translate_fail(
                        "bridge: OR branch lowered to filters-only query, which the active LQ contract cannot represent",
                    ));
                }
                out.push(lowered);
            }
            Ok(collapse_exprs(out, false))
        }
        LqDirective::Not(inner) => {
            let lowered = lower_bridge_expr(*inner, query, false)?;
            if matches!(lowered, LqExpr::Empty) {
                return Err(BridgeError::translate_fail(
                    "bridge: NOT over filters-only subtree cannot be lowered into the active LQ contract",
                ));
            }
            Ok(LqExpr::Not(Box::new(lowered)))
        }
        LqDirective::Filtered { filters, body } => {
            if !allow_scoped_filters {
                return Err(BridgeError::translate_fail(
                    "bridge: scoped filters under OR/NOT are not representable on the active LQ wire",
                ));
            }
            for filter in filters {
                let lowered = lower_bridge_expr(filter, query, true)?;
                if !matches!(lowered, LqExpr::Empty) {
                    return Err(BridgeError::translate_fail(
                        "bridge: expected filter-only bridge node while lowering filtered subtree",
                    ));
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

fn lower_bridge_pattern(kind: &str, body: &str) -> Result<LqExpr, BridgeError> {
    let leaf = match kind {
        "literal" | "keyword" => LqLeaf::Keyword(body.to_string()),
        "phrase" => LqLeaf::Phrase(body.to_string()),
        "regex" => LqLeaf::Regex(body.to_string()),
        other => {
            return Err(BridgeError::translate_fail(format!(
                "bridge: unsupported lowered pattern kind `{other}`"
            )));
        }
    };
    Ok(LqExpr::Leaf(leaf))
}

enum BridgePredicateArg {
    Phrase(String),
    RawString(String),
    Bare(String),
}

fn lower_bridge_predicate(name: &str, args_raw: &str) -> Result<LqExpr, BridgeError> {
    let args = parse_bridge_predicate_args(args_raw)?;
    if let Some(executable) = lower_executable_bridge_predicate(name, &args) {
        return Ok(executable);
    }
    Ok(LqExpr::Leaf(LqLeaf::Predicate {
        name: name.to_string(),
        args: args.into_iter().map(to_lq_predicate_arg).collect(),
    }))
}

fn lower_executable_bridge_predicate(name: &str, args: &[BridgePredicateArg]) -> Option<LqExpr> {
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
        BridgePredicateArg::RawString(text) => Some(LqExpr::Leaf(LqLeaf::RawString(text.clone()))),
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
    text.strip_prefix('/')
        .and_then(|trimmed| trimmed.strip_suffix('/'))
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

fn parse_bridge_predicate_args(raw: &str) -> Result<Vec<BridgePredicateArg>, BridgeError> {
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
            Some(b',') => pos = pos.saturating_add(1),
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
) -> Result<(usize, String), BridgeError> {
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

fn read_bare_bridge_arg(bytes: &[u8], start: usize) -> Result<(usize, String), BridgeError> {
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

fn next_utf8_char(bytes: &[u8]) -> Result<char, BridgeError> {
    let text = core::str::from_utf8(bytes).map_err(|err| {
        BridgeError::translate_fail(format!(
            "bridge: invalid UTF-8 in predicate argument: {err}"
        ))
    })?;
    text.chars()
        .next()
        .ok_or_else(|| predicate_arg_error("bridge: invalid UTF-8 in predicate argument"))
}

fn predicate_arg_error(message: &str) -> BridgeError {
    BridgeError::translate_fail(message)
}

fn apply_bridge_filter(name: &str, value: &str, query: &mut LqQuery) -> Result<(), BridgeError> {
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
        "rev" => query.filters.push(LqFilter::Rev {
            spec: value.to_string(),
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
                    return Err(BridgeError::new(
                        BridgeErrorCode::BridgeUnsupportedFilter,
                        Some(format!("type:{other}").into_boxed_str()),
                        format!("bridge: unsupported Sourcegraph type filter `{other}`"),
                    ));
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
                    return Err(BridgeError::new(
                        BridgeErrorCode::BridgeUnsupportedFilter,
                        Some(format!("select:{other}").into_boxed_str()),
                        format!("bridge: unsupported Sourcegraph select filter `{other}`"),
                    ));
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
                    return Err(BridgeError::new(
                        BridgeErrorCode::BridgeUnsupportedFilter,
                        Some(format!("case:{other}").into_boxed_str()),
                        format!("bridge: unsupported Sourcegraph case filter `{other}`"),
                    ));
                }
            });
        }
        "count" => {
            query.options.count = Some(if value == "all" {
                LqCountBound::All
            } else {
                let parsed = value.parse::<u32>().map_err(|err| {
                    BridgeError::translate_fail(format!("bridge: invalid count `{value}`: {err}"))
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
                    return Err(BridgeError::new(
                        BridgeErrorCode::BridgeUnsupportedFilter,
                        Some(format!("patterntype:{other}").into_boxed_str()),
                        format!("bridge: unsupported patterntype `{other}`"),
                    ));
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
            return Err(BridgeError::unsupported_filter(
                other,
                format!("bridge: unsupported filter `{other}`"),
            ));
        }
    }
    Ok(())
}

fn lower_yes_no_only_filter(name: &str, value: &str) -> Result<LqYesNoOnly, BridgeError> {
    match value {
        "yes" => Ok(LqYesNoOnly::Yes),
        "no" => Ok(LqYesNoOnly::No),
        "only" => Ok(LqYesNoOnly::Only),
        other => Err(BridgeError::new(
            BridgeErrorCode::BridgeUnsupportedFilter,
            Some(format!("{name}:{other}").into_boxed_str()),
            format!("bridge: unsupported Sourcegraph {name} filter `{other}`"),
        )),
    }
}

fn lower_visibility_filter(value: &str) -> Result<LqFilter, BridgeError> {
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
        other => Err(BridgeError::new(
            BridgeErrorCode::BridgeUnsupportedFilter,
            Some(format!("visibility:{other}").into_boxed_str()),
            format!("bridge: unsupported Sourcegraph visibility filter `{other}`"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{LqDirective, translate_placeholder};
    use crate::errors::BridgeErrorCode;
    use crate::syntax::{SgFilter, SgPatternKind, SgQuery, parse_sourcegraph};
    use crate::version::SourcegraphVersionTag;

    /// Resolve the supported-pin tag for tests.
    ///
    /// Falls back to a shape-validating literal if the workspace pin
    /// ever drifts; in either case returns a `SourcegraphVersionTag`
    /// so callers can stay on a `()` return type and avoid
    /// `clippy::panic_in_result_fn`.
    fn ver() -> SourcegraphVersionTag {
        // Try the supported pin first; fall back to a shape-equivalent
        // literal if the workspace pin ever drifts. The assertion in
        // the fallback halts the test process, so the inner `loop`
        // arm is provably unreachable in a sane harness but satisfies
        // the type checker without calling any disallowed
        // `unwrap`/`unwrap_or_else` helper.
        if let Ok(t) = SourcegraphVersionTag::supported() {
            return t;
        }
        assert!(false, "supported pin must parse");
        if let Ok(t) = SourcegraphVersionTag::new("sg-0.0.0") {
            return t;
        }
        loop {
            core::hint::spin_loop();
        }
    }

    fn parse(s: &str) -> SgQuery {
        match parse_sourcegraph(s) {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "parse failed for `{s}`: {e}");
                SgQuery::Pattern {
                    kind: SgPatternKind::Literal,
                    body: Box::<str>::from(""),
                }
            }
        }
    }

    fn run(sg: &str) -> LqDirective {
        let q = parse(sg);
        let v = ver();
        match translate_placeholder(q, &v) {
            Ok(d) => d,
            Err(e) => {
                assert!(false, "translate failed for `{sg}`: {e}");
                LqDirective::Pattern {
                    kind: Box::<str>::from("literal"),
                    body: Box::<str>::from(""),
                }
            }
        }
    }

    #[test]
    fn adopted_repo_lowers_one_to_one() {
        let lq = run("repo:acme/foo");
        let LqDirective::Filtered { filters, body } = lq else {
            assert!(false, "expected Filtered");
            return;
        };
        assert_eq!(filters.len(), 1);
        let Some(LqDirective::Filter { name, value }) = filters.first() else {
            assert!(false, "expected Filter");
            return;
        };
        assert_eq!(&**name, "repo");
        assert_eq!(&**value, "acme/foo");
        let LqDirective::Pattern { kind, body } = *body else {
            assert!(false, "expected Pattern body");
            return;
        };
        assert_eq!(&*kind, "literal");
        assert_eq!(&*body, "");
    }

    #[test]
    fn adopted_filter_set_each_maps() {
        for (sg, name) in [
            ("repo:x foo", "repo"),
            ("file:x foo", "file"),
            ("path:^src/ foo", "path"),
            ("lang:rust foo", "lang"),
            ("rev:refs/heads/main foo", "rev"),
            ("type:symbol foo", "type"),
            ("case:yes foo", "case"),
            ("select:repo foo", "select"),
            ("count:100 foo", "count"),
            ("patterntype:literal foo", "patterntype"),
        ] {
            let lq = run(sg);
            let LqDirective::Filtered { filters, .. } = lq else {
                assert!(false, "{sg}: expected Filtered");
                continue;
            };
            let Some(LqDirective::Filter { name: n, .. }) = filters.first() else {
                assert!(false, "{sg}: expected Filter");
                continue;
            };
            assert_eq!(&**n, name);
        }
    }

    #[test]
    fn adopted_fork_yes_is_preserved() {
        let lq = run("fork:yes foo");
        let LqDirective::Filtered { filters, .. } = lq else {
            assert!(false, "expected Filtered");
            return;
        };
        let Some(LqDirective::Filter { name, value }) = filters.first() else {
            assert!(false, "expected Filter");
            return;
        };
        assert_eq!(&**name, "fork");
        assert_eq!(&**value, "yes");
    }

    #[test]
    fn adopted_fork_no_and_only() {
        for (sg, expect) in [("fork:no foo", "no"), ("fork:only foo", "only")] {
            let lq = run(sg);
            let LqDirective::Filtered { filters, .. } = lq else {
                assert!(false, "{sg}: expected Filtered");
                continue;
            };
            let Some(LqDirective::Filter { value, .. }) = filters.first() else {
                assert!(false, "{sg}: expected Filter");
                continue;
            };
            assert_eq!(&**value, expect);
        }
    }

    #[test]
    fn adopted_archived_values() {
        for (sg, expect) in [
            ("archived:yes foo", "yes"),
            ("archived:no foo", "no"),
            ("archived:only foo", "only"),
        ] {
            let lq = run(sg);
            let LqDirective::Filtered { filters, .. } = lq else {
                assert!(false, "{sg}: expected Filtered");
                continue;
            };
            let Some(LqDirective::Filter { name, value }) = filters.first() else {
                assert!(false, "{sg}: expected Filter");
                continue;
            };
            assert_eq!(&**name, "archived");
            assert_eq!(&**value, expect);
        }
    }

    #[test]
    fn refused_fork_value() {
        let q = parse("fork:maybe foo");
        let v = ver();
        match translate_placeholder(q, &v) {
            Ok(_) => assert!(false, "fork:maybe must refuse"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedDirective),
        }
    }

    #[test]
    fn normalized_content_becomes_pattern() {
        let lq = run("content:hello");
        let LqDirective::Pattern { kind, body } = lq else {
            assert!(false, "expected Pattern");
            return;
        };
        assert_eq!(&*kind, "literal");
        assert_eq!(&*body, "hello");
    }

    #[test]
    fn content_with_other_filter_pattern_attaches_to_body() {
        let lq = run("repo:acme content:hello");
        let LqDirective::Filtered { filters, body } = lq else {
            assert!(false, "expected Filtered");
            return;
        };
        assert_eq!(filters.len(), 1);
        let LqDirective::Pattern { body, .. } = *body else {
            assert!(false, "expected Pattern body");
            return;
        };
        assert_eq!(&*body, "hello");
    }

    #[test]
    fn index_no_is_refused_but_yes_and_only_are_dropped() {
        let q = parse("index:no foo");
        let v = ver();
        match translate_placeholder(q, &v) {
            Ok(_) => assert!(false, "index: must refuse"),
            Err(e) => {
                assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedDirective);
                match e.source_construct.as_deref() {
                    Some(s) => assert!(s.starts_with("index:")),
                    None => assert!(false, "expected source_construct"),
                }
            }
        }

        for sg in ["index:yes foo", "index:only foo"] {
            let lowered = run(sg);
            let LqDirective::Pattern { body, .. } = lowered else {
                assert!(false, "{sg}: expected body pattern after index no-op");
                continue;
            };
            assert_eq!(&*body, "foo");
        }
    }

    #[test]
    fn context_is_preserved_as_filter() {
        let lowered = run("context:global foo");
        let LqDirective::Filtered { filters, .. } = lowered else {
            assert!(false, "expected Filtered");
            return;
        };
        let Some(LqDirective::Filter { name, value }) = filters.first() else {
            assert!(false, "expected Filter");
            return;
        };
        assert_eq!(&**name, "context");
        assert_eq!(&**value, "global");
    }

    #[test]
    fn boost_and_timeout_are_typed_refusals() {
        for (sg, construct) in [("boost:5 foo", "boost:5"), ("timeout:1s foo", "timeout:1s")] {
            let q = parse(sg);
            let v = ver();
            match translate_placeholder(q, &v) {
                Ok(_) => assert!(false, "{sg} must refuse"),
                Err(e) => {
                    assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedDirective);
                    match e.source_construct.as_deref() {
                        Some(s) => assert_eq!(s, construct),
                        None => assert!(false, "expected source_construct"),
                    }
                }
            }
        }
    }

    #[test]
    fn visibility_values_are_validated_and_preserved() {
        for value in ["public", "private", "any"] {
            let lowered = run(&format!("visibility:{value} foo"));
            let LqDirective::Filtered { filters, .. } = lowered else {
                assert!(false, "expected Filtered");
                continue;
            };
            let Some(LqDirective::Filter { name, value: got }) = filters.first() else {
                assert!(false, "expected Filter");
                continue;
            };
            assert_eq!(&**name, "visibility");
            assert_eq!(&**got, value);
        }
    }

    #[test]
    fn invalid_visibility_value_is_refused() {
        let q = parse("visibility:team foo");
        match translate_placeholder(q, &ver()) {
            Ok(_) => assert!(false, "visibility:team must refuse"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedDirective),
        }
    }

    #[test]
    fn fork_archived_visibility_and_context_are_all_preserved() {
        let q = SgQuery::Filtered {
            filters: vec![
                SgFilter::Fork(Box::<str>::from("yes")),
                SgFilter::Archived(Box::<str>::from("no")),
                SgFilter::Visibility(Box::<str>::from("public")),
                SgFilter::Context(Box::<str>::from("global")),
            ],
            body: Box::new(SgQuery::Pattern {
                kind: SgPatternKind::Literal,
                body: Box::<str>::from("foo"),
            }),
        };
        let lowered = match translate_placeholder(q, &ver()) {
            Ok(lowered) => lowered,
            Err(e) => {
                assert!(false, "all scope filters should lower: {e}");
                return;
            }
        };
        let LqDirective::Filtered { filters, .. } = lowered else {
            assert!(false, "expected Filtered");
            return;
        };
        assert_eq!(filters.len(), 4);
    }

    #[test]
    fn repo_predicate_lowers_to_predicate_directive() {
        let q = parse("repo:has.file(path:src/lib.rs)");
        let lowered = match translate_placeholder(q, &ver()) {
            Ok(lowered) => lowered,
            Err(e) => {
                assert!(false, "repo predicate must lower, got {e}");
                return;
            }
        };
        let LqDirective::Predicate { name, args_raw } = lowered else {
            assert!(false, "expected Predicate directive");
            return;
        };
        assert_eq!(&*name, "repo.has.file");
        assert_eq!(&*args_raw, "path:src/lib.rs");
    }

    #[test]
    fn file_predicate_lowers_to_predicate_directive() {
        let q = parse(r#"file:contains("TODO")"#);
        let lowered = match translate_placeholder(q, &ver()) {
            Ok(lowered) => lowered,
            Err(e) => {
                assert!(false, "file predicate must lower, got {e}");
                return;
            }
        };
        let LqDirective::Predicate { name, args_raw } = lowered else {
            assert!(false, "expected Predicate directive");
            return;
        };
        assert_eq!(&*name, "file.contains");
        assert_eq!(&*args_raw, r#""TODO""#);
    }

    #[test]
    fn boolean_or_translates_recursively() {
        let lq = run("foo OR bar");
        let LqDirective::Or(xs) = lq else {
            assert!(false, "expected Or");
            return;
        };
        assert_eq!(xs.len(), 2);
    }

    #[test]
    fn boolean_not_translates_recursively() {
        let lq = run("NOT foo");
        let LqDirective::Not(inner) = lq else {
            assert!(false, "expected Not");
            return;
        };
        let LqDirective::Pattern { body, .. } = *inner else {
            assert!(false, "expected Pattern inside Not");
            return;
        };
        assert_eq!(&*body, "foo");
    }

    #[test]
    fn phrase_and_regex_preserve_pattern_kind() {
        let phrase = run("\"hello world\"");
        let LqDirective::Pattern { kind, body } = phrase else {
            assert!(false, "expected phrase pattern");
            return;
        };
        assert_eq!(&*kind, "phrase");
        assert_eq!(&*body, "hello world");

        let regex = run("/h.llo/");
        let LqDirective::Pattern { kind, body } = regex else {
            assert!(false, "expected regex pattern");
            return;
        };
        assert_eq!(&*kind, "regex");
        assert_eq!(&*body, "h.llo");
    }

    #[test]
    fn identical_filtered_or_branches_are_hoisted() {
        let lowered = run("repo:acme/foo alpha OR repo:acme/foo beta");
        let LqDirective::Filtered { filters, body } = lowered else {
            assert!(false, "expected hoisted Filtered");
            return;
        };
        assert_eq!(filters.len(), 1);
        let Some(LqDirective::Filter { name, value }) = filters.first() else {
            assert!(false, "expected shared filter");
            return;
        };
        assert_eq!(&**name, "repo");
        assert_eq!(&**value, "acme/foo");
        let LqDirective::Or(branches) = *body else {
            assert!(false, "expected OR body");
            return;
        };
        assert_eq!(branches.len(), 2);
    }
}
