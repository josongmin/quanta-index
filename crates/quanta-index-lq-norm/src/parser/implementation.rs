//! LQ recursive-descent parser.
//!
//! Consumes the `LqToken` sequence emitted by [`crate::tokenizer::tokenize`]
//! and produces an [`LqNormalizedQuery`] (pre-normalize — the normalizer
//! pass is a separate step). Boolean precedence is `NOT > AND > OR` per
//! dsl.md §3.2; explicit groups via `(...)`. AST depth tracker is checked
//! at every recursion against [`crate::limits::MAX_AST_DEPTH`] before the
//! recursive call descends.

use crate::ast::{
    LQ_VERSION_TAG, LqCase, LqCountBound, LqDirective, LqExpr, LqFileScope, LqFilter, LqLeaf,
    LqMetaVar, LqNormalizedQuery, LqOptions, LqPatternType, LqPredicateArg, LqSelect,
    LqStructuralBlock, LqStructuralConstraint, LqStructuralConstraintOperand, LqStructuralExpr,
    LqStructuralHoleMultiplicity, LqStructuralHoleRef, LqStructuralNode, LqType, LqVisibility,
    LqYesNoOnly,
};
use crate::errors::{LqParseError, LqParseErrorCode, LqSpan};
use crate::limits::{MAX_AST_DEPTH, MAX_FANOUT_PER_NODE};
use crate::tokenizer::{LqToken, LqTokenKind};

/// Parse a tokenized input into an `LqNormalizedQuery`.
///
/// Does not invoke the normalizer; callers run `normalize(parse(tokens, input)?)`
/// for the canonical pipeline. The intermediate AST is internally already
/// shaped in canonical form (n-ary All/Any at boolean joins, dedicated
/// filter / directive / option slots) so the normalizer is a fixed-point
/// transform rather than a structural rewrite.
pub fn parse(tokens: &[LqToken], input: &str) -> Result<LqNormalizedQuery, LqParseError> {
    let total_len = u32::try_from(input.len()).map_err(|_e| {
        LqParseError::new(
            LqParseErrorCode::LimitExceededBytes,
            LqSpan::new(0, 0),
            "input length exceeds u32",
        )
    })?;
    let mut p = Parser {
        tokens,
        cursor: 0,
        filters: Vec::new(),
        directives: Vec::new(),
        options: LqOptions::defaults(),
        seen_type: false,
        seen_pattern_type: false,
    };
    let expr = p.parse_or_expression(0)?;
    if !matches!(p.peek_kind(), LqTokenKind::Eof) {
        let span = p.peek_span();
        return Err(LqParseError::new(
            LqParseErrorCode::SyntaxError,
            span,
            "extra tokens after expression",
        ));
    }
    Ok(LqNormalizedQuery {
        lq_version: LQ_VERSION_TAG,
        expr,
        filters: p.filters,
        directives: p.directives,
        options: p.options,
        source_span: LqSpan::new(0, total_len),
    })
}

struct Parser<'a> {
    tokens: &'a [LqToken],
    cursor: usize,
    filters: Vec<LqFilter>,
    directives: Vec<LqDirective>,
    options: LqOptions,
    seen_type: bool,
    seen_pattern_type: bool,
}

impl Parser<'_> {
    fn peek_kind(&self) -> &LqTokenKind {
        self.tokens
            .get(self.cursor)
            .map_or(&LqTokenKind::Eof, |t| &t.kind)
    }

    fn peek_span(&self) -> LqSpan {
        self.tokens
            .get(self.cursor)
            .map_or_else(|| LqSpan::new(0, 0), |t| t.span)
    }

    /// Advance the cursor by one and discard the token reference.
    fn consume(&mut self) {
        self.cursor = self.cursor.saturating_add(1);
    }

    fn check_depth(&self, depth: u32) -> Result<(), LqParseError> {
        if depth >= MAX_AST_DEPTH {
            Err(LqParseError::new(
                LqParseErrorCode::LimitExceededDepth,
                self.peek_span(),
                "AST depth exceeds 32",
            ))
        } else {
            Ok(())
        }
    }

    fn parse_or_expression(&mut self, depth: u32) -> Result<LqExpr, LqParseError> {
        self.check_depth(depth)?;
        let next_depth = depth.saturating_add(1);
        let mut children: Vec<LqExpr> = Vec::new();
        let first = self.parse_and_expression(next_depth)?;
        children.push(first);
        while matches!(self.peek_kind(), LqTokenKind::Or) {
            self.consume();
            let rhs = self.parse_and_expression(next_depth)?;
            children.push(rhs);
        }
        if children.len() == 1 {
            // Safety: len == 1 by check above.
            let Some(single) = children.into_iter().next() else {
                return Err(LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    self.peek_span(),
                    "internal: empty OR children",
                ));
            };
            return Ok(single);
        }
        if children.len() > MAX_FANOUT_PER_NODE {
            return Err(LqParseError::new(
                LqParseErrorCode::LimitExceededFanout,
                self.peek_span(),
                "OR fan-out exceeds 64",
            ));
        }
        Ok(LqExpr::Any(children))
    }

    fn parse_and_expression(&mut self, depth: u32) -> Result<LqExpr, LqParseError> {
        self.check_depth(depth)?;
        let next_depth = depth.saturating_add(1);
        let mut children: Vec<LqExpr> = Vec::new();
        loop {
            // Stop conditions: EOF, OR, RParen.
            match self.peek_kind() {
                LqTokenKind::Eof | LqTokenKind::Or | LqTokenKind::RParen => break,
                LqTokenKind::And => {
                    // Explicit AND between atoms — consume and require a
                    // right-hand atom on the next iteration. If the loop
                    // would terminate after the consume (no more atoms),
                    // that's a trailing-operator syntax error.
                    if children.is_empty() {
                        return Err(LqParseError::new(
                            LqParseErrorCode::SyntaxError,
                            self.peek_span(),
                            "AND without left operand",
                        ));
                    }
                    self.consume();
                    let expected_child_count = children.len().saturating_add(1);
                    let nxt = self.parse_not_or_atom(next_depth)?;
                    let Some(nxt) = nxt else {
                        return Err(LqParseError::new(
                            LqParseErrorCode::SyntaxError,
                            self.peek_span(),
                            "AND without right operand",
                        ));
                    };
                    children.push(nxt);
                    if children.len() != expected_child_count {
                        return Err(LqParseError::new(
                            LqParseErrorCode::SyntaxError,
                            self.peek_span(),
                            "AND right operand missing",
                        ));
                    }
                    continue;
                }
                LqTokenKind::KeywordOrFilterName(_)
                | LqTokenKind::Phrase(_)
                | LqTokenKind::RawString(_)
                | LqTokenKind::Regex(_)
                | LqTokenKind::StructuralBlock(_)
                | LqTokenKind::ColonValue(_)
                | LqTokenKind::Predicate { .. }
                | LqTokenKind::Colon
                | LqTokenKind::Not
                | LqTokenKind::Dash
                | LqTokenKind::LParen => {}
            }
            let starting_cursor = self.cursor;
            let child_opt = self.parse_not_or_atom(next_depth)?;
            if let Some(child) = child_opt {
                children.push(child);
            } else if self.cursor == starting_cursor {
                // No progress made and no child emitted — break to avoid loop.
                break;
            }
        }
        if children.is_empty() {
            return Ok(LqExpr::Empty);
        }
        if children.len() == 1 {
            let Some(single) = children.into_iter().next() else {
                return Err(LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    self.peek_span(),
                    "internal: empty AND children",
                ));
            };
            return Ok(single);
        }
        if children.len() > MAX_FANOUT_PER_NODE {
            return Err(LqParseError::new(
                LqParseErrorCode::LimitExceededFanout,
                self.peek_span(),
                "AND fan-out exceeds 64",
            ));
        }
        Ok(LqExpr::All(children))
    }

    /// Parse a NOT expression, dash-negation, atom, or filter. Returns `Ok(None)`
    /// when the consumed tokens are non-expression (filter / directive / option).
    fn parse_not_or_atom(&mut self, depth: u32) -> Result<Option<LqExpr>, LqParseError> {
        self.check_depth(depth)?;
        let next_depth = depth.saturating_add(1);
        match self.peek_kind() {
            LqTokenKind::Not | LqTokenKind::Dash => {
                self.consume();
                let inner_opt = self.parse_not_or_atom(next_depth)?;
                let Some(inner) = inner_opt else {
                    return Err(LqParseError::new(
                        LqParseErrorCode::SyntaxError,
                        self.peek_span(),
                        "NOT/- without expression",
                    ));
                };
                Ok(Some(LqExpr::Not(Box::new(inner))))
            }
            LqTokenKind::LParen => {
                self.consume();
                let inner = self.parse_or_expression(next_depth)?;
                if !matches!(self.peek_kind(), LqTokenKind::RParen) {
                    return Err(LqParseError::new(
                        LqParseErrorCode::SyntaxError,
                        self.peek_span(),
                        "expected )",
                    ));
                }
                self.consume();
                Ok(Some(inner))
            }
            LqTokenKind::Phrase(_)
            | LqTokenKind::RawString(_)
            | LqTokenKind::Regex(_)
            | LqTokenKind::StructuralBlock(_)
            | LqTokenKind::KeywordOrFilterName(_)
            | LqTokenKind::Predicate { .. } => {
                let leaf = self.parse_leaf_or_filter()?;
                Ok(leaf)
            }
            LqTokenKind::Eof
            | LqTokenKind::Or
            | LqTokenKind::And
            | LqTokenKind::RParen
            | LqTokenKind::Colon
            | LqTokenKind::ColonValue(_) => Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                self.peek_span(),
                "unexpected token in atom position",
            )),
        }
    }

    /// Parse a leaf or a filter. Filters are absorbed into `self.filters`,
    /// directives into `self.directives`, options into `self.options`; only
    /// non-filter leaves return `Some(LqExpr::Leaf(_))`.
    fn parse_leaf_or_filter(&mut self) -> Result<Option<LqExpr>, LqParseError> {
        let head_tok = self.tokens.get(self.cursor).cloned();
        let Some(head) = head_tok else {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                LqSpan::new(0, 0),
                "expected leaf or filter, got EOF",
            ));
        };
        self.cursor = self.cursor.saturating_add(1);
        match head.kind {
            LqTokenKind::Phrase(s) => Ok(Some(LqExpr::Leaf(LqLeaf::Phrase(s)))),
            LqTokenKind::RawString(s) => Ok(Some(LqExpr::Leaf(LqLeaf::RawString(s)))),
            LqTokenKind::Regex(s) => {
                crate::regex_guard::precheck_regex(&s, head.span)?;
                Ok(Some(LqExpr::Leaf(LqLeaf::Regex(s))))
            }
            LqTokenKind::StructuralBlock(raw) => {
                let block = parse_structural_body(&raw, head.span)?;
                Ok(Some(LqExpr::Leaf(LqLeaf::StructuralBlock(block))))
            }
            LqTokenKind::KeywordOrFilterName(name) => {
                if matches!(self.peek_kind(), LqTokenKind::Colon) {
                    self.consume();
                    let value_tok = self.tokens.get(self.cursor).cloned();
                    let Some(value) = value_tok else {
                        return Err(LqParseError::new(
                            LqParseErrorCode::InvalidFilterValue,
                            head.span,
                            "missing filter value",
                        ));
                    };
                    self.cursor = self.cursor.saturating_add(1);
                    match value.kind {
                        LqTokenKind::ColonValue(val) => {
                            self.absorb_filter_or_option(&name, &val, head.span)?;
                            Ok(None)
                        }
                        LqTokenKind::Predicate {
                            name: predicate_name,
                            args_raw,
                        } => {
                            let canonical_name = format!("{name}.{predicate_name}");
                            let args = parse_predicate_args(&args_raw, value.span)?;
                            Ok(Some(LqExpr::Leaf(LqLeaf::Predicate {
                                name: canonical_name,
                                args,
                            })))
                        }
                        LqTokenKind::Phrase(_)
                        | LqTokenKind::RawString(_)
                        | LqTokenKind::Regex(_)
                        | LqTokenKind::StructuralBlock(_)
                        | LqTokenKind::KeywordOrFilterName(_)
                        | LqTokenKind::And
                        | LqTokenKind::Or
                        | LqTokenKind::Not
                        | LqTokenKind::Dash
                        | LqTokenKind::LParen
                        | LqTokenKind::RParen
                        | LqTokenKind::Colon
                        | LqTokenKind::Eof => Err(LqParseError::new(
                            LqParseErrorCode::InvalidFilterValue,
                            value.span,
                            "missing filter value",
                        )),
                    }
                } else {
                    Ok(Some(LqExpr::Leaf(LqLeaf::Keyword(name))))
                }
            }
            LqTokenKind::Predicate { name, args_raw } => {
                let args = parse_predicate_args(&args_raw, head.span)?;
                Ok(Some(LqExpr::Leaf(LqLeaf::Predicate { name, args })))
            }
            LqTokenKind::And
            | LqTokenKind::Or
            | LqTokenKind::Not
            | LqTokenKind::Dash
            | LqTokenKind::LParen
            | LqTokenKind::RParen
            | LqTokenKind::Colon
            | LqTokenKind::ColonValue(_)
            | LqTokenKind::Eof => Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                head.span,
                "unexpected token in leaf position",
            )),
        }
    }

    fn absorb_filter_or_option(
        &mut self,
        name: &str,
        value: &str,
        span: LqSpan,
    ) -> Result<(), LqParseError> {
        // dsl.md §6.1: filter names are case-insensitive at lookup; lower-case
        // canonically. (Default position recorded in PRE-NORM.md §12.)
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "repo" => {
                let (pat, revs) = parse_repo_value(value);
                self.filters.push(LqFilter::Repo { pattern: pat, revs });
                Ok(())
            }
            "file" => {
                self.filters.push(LqFilter::File {
                    pattern: value.to_owned(),
                    scope: LqFileScope::NameAndPath,
                });
                Ok(())
            }
            "path" => {
                self.filters.push(LqFilter::File {
                    pattern: value.to_owned(),
                    scope: LqFileScope::PathOnly,
                });
                Ok(())
            }
            "lang" => {
                self.filters.push(LqFilter::Lang {
                    id: value.to_owned(),
                });
                Ok(())
            }
            "rev" => {
                self.filters.push(LqFilter::Rev {
                    spec: value.to_owned(),
                });
                Ok(())
            }
            "author" => {
                self.filters.push(LqFilter::Author {
                    pattern: value.to_owned(),
                });
                Ok(())
            }
            "committer" => {
                self.filters.push(LqFilter::Committer {
                    pattern: value.to_owned(),
                });
                Ok(())
            }
            "message" => {
                self.filters.push(LqFilter::Message {
                    pattern: value.to_owned(),
                });
                Ok(())
            }
            "before" => {
                self.filters.push(LqFilter::Before {
                    timeref: value.to_owned(),
                });
                Ok(())
            }
            "after" => {
                self.filters.push(LqFilter::After {
                    timeref: value.to_owned(),
                });
                Ok(())
            }
            "since" => {
                self.filters.push(LqFilter::Since {
                    timeref: value.to_owned(),
                });
                Ok(())
            }
            "since.time" => {
                self.filters.push(LqFilter::Since {
                    timeref: format!("time:{value}"),
                });
                Ok(())
            }
            "since.commit" => {
                self.filters.push(LqFilter::Since {
                    timeref: format!("commit:{value}"),
                });
                Ok(())
            }
            "until" => {
                self.filters.push(LqFilter::Until {
                    timeref: value.to_owned(),
                });
                Ok(())
            }
            "diff.added" => {
                self.filters.push(LqFilter::DiffAdded {
                    pattern: value.to_owned(),
                });
                Ok(())
            }
            "diff.removed" => {
                self.filters.push(LqFilter::DiffRemoved {
                    pattern: value.to_owned(),
                });
                Ok(())
            }
            "diff.touched" => {
                self.filters.push(LqFilter::DiffTouched {
                    pattern: value.to_owned(),
                });
                Ok(())
            }
            "type" => {
                if self.seen_type {
                    return Err(LqParseError::new(
                        LqParseErrorCode::InvalidFilterValue,
                        span,
                        "duplicate type: filter",
                    ));
                }
                self.seen_type = true;
                let kind = parse_type_value(value).ok_or_else(|| {
                    LqParseError::new(
                        LqParseErrorCode::InvalidFilterValue,
                        span,
                        "type: value not in {file,path,symbol,commit,diff,repo}",
                    )
                })?;
                self.filters.push(LqFilter::Type { kind });
                Ok(())
            }
            "select" => {
                let dim = parse_select_value(value).ok_or_else(|| {
                    LqParseError::new(
                        LqParseErrorCode::InvalidFilterValue,
                        span,
                        "select: value not in closed set",
                    )
                })?;
                self.filters.push(LqFilter::Select { dim });
                Ok(())
            }
            "dirty" => {
                let mode = parse_yes_no_only(value).ok_or_else(|| {
                    LqParseError::new(
                        LqParseErrorCode::InvalidFilterValue,
                        span,
                        "dirty: value not in {yes,no,only}",
                    )
                })?;
                self.filters.push(LqFilter::Dirty { mode });
                Ok(())
            }
            "changed" => {
                self.filters.push(LqFilter::Changed {
                    scope: value.to_owned(),
                });
                Ok(())
            }
            "stale" => {
                self.filters.push(LqFilter::Stale {
                    scope: value.to_owned(),
                });
                Ok(())
            }
            "snapshot" => {
                self.filters.push(LqFilter::Snapshot {
                    name: value.to_owned(),
                });
                Ok(())
            }
            "meta.owner" => {
                self.filters.push(LqFilter::MetaOwner {
                    id: value.to_owned(),
                });
                Ok(())
            }
            "meta.service" => {
                self.filters.push(LqFilter::MetaService {
                    id: value.to_owned(),
                });
                Ok(())
            }
            "meta.layer" => {
                self.filters.push(LqFilter::MetaLayer {
                    id: value.to_owned(),
                });
                Ok(())
            }
            "meta.surface" => {
                self.filters.push(LqFilter::MetaSurface {
                    id: value.to_owned(),
                });
                Ok(())
            }
            "affected" => {
                self.filters.push(LqFilter::Affected {
                    scope: value.to_owned(),
                });
                Ok(())
            }
            "invalidated_by" => {
                self.filters.push(LqFilter::InvalidatedBy {
                    source: value.to_owned(),
                });
                Ok(())
            }
            "fork" => {
                let mode = parse_yes_no_only(value).ok_or_else(|| {
                    LqParseError::new(
                        LqParseErrorCode::InvalidFilterValue,
                        span,
                        "fork: value not in {yes,no,only}",
                    )
                })?;
                self.filters.push(LqFilter::Fork { mode });
                Ok(())
            }
            "archived" => {
                let mode = parse_yes_no_only(value).ok_or_else(|| {
                    LqParseError::new(
                        LqParseErrorCode::InvalidFilterValue,
                        span,
                        "archived: value not in {yes,no,only}",
                    )
                })?;
                self.filters.push(LqFilter::Archived { mode });
                Ok(())
            }
            "visibility" => {
                let mode = parse_visibility(value).ok_or_else(|| {
                    LqParseError::new(
                        LqParseErrorCode::InvalidFilterValue,
                        span,
                        "visibility: value not in {public,private,any}",
                    )
                })?;
                self.filters.push(LqFilter::Visibility { mode });
                Ok(())
            }
            "context" => {
                self.filters.push(LqFilter::Context {
                    name: value.to_owned(),
                });
                Ok(())
            }
            "content" => {
                self.filters.push(LqFilter::Content {
                    leaf: LqLeaf::Keyword(value.to_owned()),
                });
                Ok(())
            }
            "case" => {
                let case = match value {
                    "yes" => LqCase::Sensitive,
                    "no" => LqCase::Insensitive,
                    _ => {
                        return Err(LqParseError::new(
                            LqParseErrorCode::InvalidFilterValue,
                            span,
                            "case: value not in {yes,no}",
                        ));
                    }
                };
                self.options.case = Some(case);
                Ok(())
            }
            "count" => {
                let count = if value == "all" {
                    LqCountBound::All
                } else {
                    let n: u32 = value.parse().map_err(|_e| {
                        LqParseError::new(
                            LqParseErrorCode::InvalidFilterValue,
                            span,
                            "count: value not an integer or 'all'",
                        )
                    })?;
                    LqCountBound::Bounded(n)
                };
                self.options.count = Some(count);
                Ok(())
            }
            "timeout" => {
                self.options.timeout_ms = Some(parse_timeout_ms(value, span)?);
                Ok(())
            }
            "patterntype" => {
                if self.seen_pattern_type {
                    return Err(LqParseError::new(
                        LqParseErrorCode::InvalidPatternType,
                        span,
                        "duplicate patterntype: filter",
                    ));
                }
                self.seen_pattern_type = true;
                let pt = parse_pattern_type(value).ok_or_else(|| {
                    LqParseError::new(
                        LqParseErrorCode::InvalidPatternType,
                        span,
                        "patterntype: value not in {literal,keyword,standard,regexp,structural}",
                    )
                })?;
                self.options.pattern_type = pt;
                Ok(())
            }
            "into" => {
                let directive = match value {
                    "codeql" => LqDirective::IntoCodeQl,
                    _ => {
                        return Err(LqParseError::new(
                            LqParseErrorCode::InvalidFilterValue,
                            span,
                            "into: value not in {codeql}",
                        ));
                    }
                };
                self.directives.push(directive);
                Ok(())
            }
            "scope" => {
                let directive = match value {
                    "results" => LqDirective::ScopeResults,
                    _ => {
                        return Err(LqParseError::new(
                            LqParseErrorCode::InvalidFilterValue,
                            span,
                            "scope: value not in {results}",
                        ));
                    }
                };
                self.directives.push(directive);
                Ok(())
            }
            "with" => {
                let directive = match value {
                    "lexical" => LqDirective::WithLexical,
                    _ => {
                        return Err(LqParseError::new(
                            LqParseErrorCode::InvalidFilterValue,
                            span,
                            "with: value not in {lexical}",
                        ));
                    }
                };
                self.directives.push(directive);
                Ok(())
            }
            _ => Err(LqParseError::new(
                LqParseErrorCode::UnknownFilter,
                span,
                "unknown filter name",
            )),
        }
    }
}

fn parse_timeout_ms(value: &str, span: LqSpan) -> Result<u64, LqParseError> {
    let (digits, unit) = split_timeout_value(value).ok_or_else(|| {
        LqParseError::new(
            LqParseErrorCode::InvalidFilterValue,
            span,
            "timeout: value must be <int><unit> with unit in {ms,s,m,h}",
        )
    })?;
    let magnitude: u64 = digits.parse().map_err(|_err| {
        LqParseError::new(
            LqParseErrorCode::InvalidFilterValue,
            span,
            "timeout: value must be <int><unit> with unit in {ms,s,m,h}",
        )
    })?;
    let multiplier: u64 = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => {
            return Err(LqParseError::new(
                LqParseErrorCode::InvalidFilterValue,
                span,
                "timeout: unit must be one of {ms,s,m,h}",
            ));
        }
    };
    magnitude.checked_mul(multiplier).ok_or_else(|| {
        LqParseError::new(
            LqParseErrorCode::InvalidFilterValue,
            span,
            "timeout: duration exceeds u64 milliseconds",
        )
    })
}

fn split_timeout_value(value: &str) -> Option<(&str, &str)> {
    let digit_len = value.bytes().take_while(u8::is_ascii_digit).count();
    if digit_len == 0 || digit_len == value.len() {
        return None;
    }
    Some(value.split_at(digit_len))
}

fn parse_repo_value(value: &str) -> (String, Vec<String>) {
    // `repo:foo@rev1,rev2` → ("foo", ["rev1","rev2"]); `repo:foo` → ("foo", []).
    if let Some((pat, rest)) = value.split_once('@') {
        let revs: Vec<String> = rest
            .split(',')
            .filter(|r| !r.is_empty())
            .map(str::to_owned)
            .collect();
        return (pat.to_owned(), revs);
    }
    (value.to_owned(), Vec::new())
}

fn parse_type_value(v: &str) -> Option<LqType> {
    let t = match v {
        "file" => LqType::File,
        "path" => LqType::Path,
        "symbol" => LqType::Symbol,
        "commit" => LqType::Commit,
        "diff" => LqType::Diff,
        "repo" => LqType::Repo,
        _ => return None,
    };
    Some(t)
}

fn parse_select_value(v: &str) -> Option<LqSelect> {
    let s = match v {
        "repo" => LqSelect::Repo,
        "file" => LqSelect::File,
        "file.owners" => LqSelect::FileOwners,
        "path" => LqSelect::Path,
        "symbol" => LqSelect::Symbol,
        "content" => LqSelect::Content,
        "content.match" => LqSelect::ContentMatch,
        _ => return None,
    };
    Some(s)
}

fn parse_yes_no_only(v: &str) -> Option<LqYesNoOnly> {
    let y = match v {
        "yes" => LqYesNoOnly::Yes,
        "no" => LqYesNoOnly::No,
        "only" => LqYesNoOnly::Only,
        _ => return None,
    };
    Some(y)
}

fn parse_visibility(v: &str) -> Option<LqVisibility> {
    let p = match v {
        "public" => LqVisibility::Public,
        "private" => LqVisibility::Private,
        "any" => LqVisibility::Any,
        _ => return None,
    };
    Some(p)
}

fn parse_pattern_type(v: &str) -> Option<LqPatternType> {
    let p = match v {
        "literal" => LqPatternType::Literal,
        "keyword" => LqPatternType::Keyword,
        "standard" => LqPatternType::Standard,
        "regexp" => LqPatternType::Regexp,
        "structural" => LqPatternType::Structural,
        _ => return None,
    };
    Some(p)
}

/// Parse the parenthesised argument body of a predicate token.
///
/// Args are comma-separated; each arg is one of:
/// - `"..."` — quoted phrase
/// - `'...'` — raw string
/// - `name:value` — nested filter shape (value runs to next `,` or end)
/// - bare token — keyword if not a number, else `Number`
///
/// Whitespace surrounding each arg is trimmed. An empty body parses to an
/// empty `args` list. Unterminated quotes surface
/// [`LqParseErrorCode::UnclosedQuote`].
fn parse_predicate_args(raw: &str, span: LqSpan) -> Result<Vec<LqPredicateArg>, LqParseError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let mut out: Vec<LqPredicateArg> = Vec::new();
    let bytes = raw.as_bytes();
    let mut pos: usize = 0;
    let len = bytes.len();
    loop {
        // Skip leading whitespace.
        while pos < len {
            let Some(&b) = bytes.get(pos) else { break };
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                let next = pos.checked_add(1).ok_or_else(|| {
                    LqParseError::new(
                        LqParseErrorCode::SyntaxError,
                        span,
                        "predicate args offset overflow",
                    )
                })?;
                pos = next;
            } else {
                break;
            }
        }
        if pos >= len {
            break;
        }
        let Some(&head) = bytes.get(pos) else {
            break;
        };
        let arg = match head {
            b'"' => {
                let (consumed, value) = read_quoted_arg(bytes, pos, b'"', span)?;
                pos = consumed;
                LqPredicateArg::Phrase(value)
            }
            b'\'' => {
                let (consumed, value) = read_quoted_arg(bytes, pos, b'\'', span)?;
                pos = consumed;
                LqPredicateArg::RawString(value)
            }
            _ => {
                // Read until `,` (top-level) or EOF.
                let (consumed, value) = read_bare_arg(bytes, pos, span)?;
                pos = consumed;
                let trimmed_val = value.trim();
                if trimmed_val.is_empty() {
                    return Err(LqParseError::new(
                        LqParseErrorCode::SyntaxError,
                        span,
                        "empty predicate argument",
                    ));
                }
                classify_bare_arg(trimmed_val)
            }
        };
        out.push(arg);
        // Skip trailing whitespace.
        while pos < len {
            let Some(&b) = bytes.get(pos) else { break };
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                let next = pos.checked_add(1).ok_or_else(|| {
                    LqParseError::new(
                        LqParseErrorCode::SyntaxError,
                        span,
                        "predicate args offset overflow",
                    )
                })?;
                pos = next;
            } else {
                break;
            }
        }
        if pos >= len {
            break;
        }
        let Some(&sep) = bytes.get(pos) else { break };
        if sep == b',' {
            let next = pos.checked_add(1).ok_or_else(|| {
                LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    span,
                    "predicate args offset overflow",
                )
            })?;
            pos = next;
            continue;
        }
        // Anything else here is a syntax error — the bare reader stops at
        // `,` only, and quoted readers stopped at the closing quote.
        return Err(LqParseError::new(
            LqParseErrorCode::SyntaxError,
            span,
            "unexpected character in predicate args",
        ));
    }
    Ok(out)
}

fn read_quoted_arg(
    bytes: &[u8],
    start: usize,
    terminator: u8,
    span: LqSpan,
) -> Result<(usize, String), LqParseError> {
    let mut pos = start.checked_add(1).ok_or_else(|| {
        LqParseError::new(
            LqParseErrorCode::SyntaxError,
            span,
            "predicate args offset overflow",
        )
    })?;
    let mut buf = String::new();
    while let Some(&b) = bytes.get(pos) {
        if b == terminator {
            let next = pos.checked_add(1).ok_or_else(|| {
                LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    span,
                    "predicate args offset overflow",
                )
            })?;
            return Ok((next, buf));
        }
        if b == b'\\' && terminator == b'"' {
            let after = pos.checked_add(1).ok_or_else(|| {
                LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    span,
                    "predicate args offset overflow",
                )
            })?;
            let Some(&esc) = bytes.get(after) else {
                return Err(LqParseError::new(
                    LqParseErrorCode::TokenInvalid,
                    span,
                    "trailing backslash in predicate phrase",
                ));
            };
            let mapped = match esc {
                b'\\' => '\\',
                b'"' => '"',
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                _ => {
                    return Err(LqParseError::new(
                        LqParseErrorCode::TokenInvalid,
                        span,
                        "unknown phrase escape in predicate arg",
                    ));
                }
            };
            buf.push(mapped);
            pos = after.checked_add(1).ok_or_else(|| {
                LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    span,
                    "predicate args offset overflow",
                )
            })?;
            continue;
        }
        let rest = bytes.get(pos..).unwrap_or(&[]);
        let ch = match core::str::from_utf8(rest) {
            Ok(s) => match s.chars().next() {
                Some(c) => c,
                None => {
                    return Err(LqParseError::new(
                        LqParseErrorCode::TokenInvalid,
                        span,
                        "invalid UTF-8 in predicate arg",
                    ));
                }
            },
            Err(_e) => {
                return Err(LqParseError::new(
                    LqParseErrorCode::TokenInvalid,
                    span,
                    "invalid UTF-8 in predicate arg",
                ));
            }
        };
        buf.push(ch);
        let step = ch.len_utf8();
        pos = pos.checked_add(step).ok_or_else(|| {
            LqParseError::new(
                LqParseErrorCode::SyntaxError,
                span,
                "predicate args offset overflow",
            )
        })?;
    }
    Err(LqParseError::new(
        LqParseErrorCode::UnclosedQuote,
        span,
        "unterminated quoted predicate arg",
    ))
}

fn read_bare_arg(
    bytes: &[u8],
    start: usize,
    span: LqSpan,
) -> Result<(usize, String), LqParseError> {
    let mut pos = start;
    let mut buf = String::new();
    while let Some(&b) = bytes.get(pos) {
        if b == b',' {
            break;
        }
        let rest = bytes.get(pos..).unwrap_or(&[]);
        let ch = match core::str::from_utf8(rest) {
            Ok(s) => match s.chars().next() {
                Some(c) => c,
                None => break,
            },
            Err(_e) => {
                return Err(LqParseError::new(
                    LqParseErrorCode::TokenInvalid,
                    span,
                    "invalid UTF-8 in predicate arg",
                ));
            }
        };
        buf.push(ch);
        let step = ch.len_utf8();
        pos = pos.checked_add(step).ok_or_else(|| {
            LqParseError::new(
                LqParseErrorCode::SyntaxError,
                span,
                "predicate args offset overflow",
            )
        })?;
    }
    Ok((pos, buf))
}

fn classify_bare_arg(s: &str) -> LqPredicateArg {
    if let Some((name, value)) = s.split_once(':') {
        return LqPredicateArg::Filter {
            name: name.to_owned(),
            value: value.to_owned(),
        };
    }
    if let Ok(n) = s.parse::<i64>() {
        return LqPredicateArg::Number(n);
    }
    LqPredicateArg::Keyword(s.to_owned())
}

/// Parse the raw body of a `match { ... }` block into a typed structural
/// block (with `lang: None` for v1) per dsl.md §8.
///
/// Caller passes the body bytes captured by the tokenizer (the segment
/// between the outer `{` and `}`). Returns:
///
/// - `SyntaxError` on unbalanced braces or malformed metavar
/// - `LimitExceededStructural` on > `MAX_STRUCTURAL_NODES` total nodes
fn parse_structural_body(raw: &str, span: LqSpan) -> Result<LqStructuralBlock, LqParseError> {
    let mut parser = StructuralParser::new(raw, span);
    let block = parser.parse_body(0, false)?;
    if parser.pos < parser.bytes.len() {
        return Err(LqParseError::new(
            LqParseErrorCode::SyntaxError,
            span,
            "trailing input after structural body",
        ));
    }
    count_structural_block(&block, span)?;
    Ok(block)
}

fn count_structural_block(block: &LqStructuralBlock, span: LqSpan) -> Result<(), LqParseError> {
    let mut node_count: u32 = 0;
    count_structural_block_with_acc(block, &mut node_count, span)
}

fn count_structural_block_with_acc(
    block: &LqStructuralBlock,
    acc: &mut u32,
    span: LqSpan,
) -> Result<(), LqParseError> {
    for expr in &block.exprs {
        count_structural_expr(expr, acc, span)?;
    }
    Ok(())
}

fn count_structural_expr(
    expr: &LqStructuralExpr,
    acc: &mut u32,
    span: LqSpan,
) -> Result<(), LqParseError> {
    match expr {
        LqStructuralExpr::Pattern(nodes) => {
            for node in nodes {
                count_structural_nodes(node, acc, span)?;
            }
            Ok(())
        }
        LqStructuralExpr::Where(_) => Ok(()),
        LqStructuralExpr::Inside(block) | LqStructuralExpr::Outside(block) => {
            count_structural_block_with_acc(block, acc, span)
        }
    }
}

fn count_structural_nodes(
    node: &LqStructuralNode,
    acc: &mut u32,
    span: LqSpan,
) -> Result<(), LqParseError> {
    let limit = crate::limits::MAX_STRUCTURAL_NODES;
    *acc = acc.checked_add(1).ok_or_else(|| {
        LqParseError::new(
            LqParseErrorCode::LimitExceededStructural,
            span,
            "structural node count overflowed u32",
        )
    })?;
    if *acc > limit {
        return Err(LqParseError::new(
            LqParseErrorCode::LimitExceededStructural,
            span,
            "structural pattern node count exceeds cap",
        ));
    }
    match node {
        LqStructuralNode::Literal(_)
        | LqStructuralNode::MetaVar(_)
        | LqStructuralNode::Hole { .. }
        | LqStructuralNode::WildcardMany => Ok(()),
        LqStructuralNode::Group(children) => {
            for c in children {
                count_structural_nodes(c, acc, span)?;
            }
            Ok(())
        }
    }
}

struct StructuralParser<'a> {
    bytes: &'a [u8],
    pos: usize,
    span: LqSpan,
}

impl<'a> StructuralParser<'a> {
    fn new(raw: &'a str, span: LqSpan) -> Self {
        Self {
            bytes: raw.as_bytes(),
            pos: 0,
            span,
        }
    }

    fn bump(&mut self) -> Result<(), LqParseError> {
        self.pos = self.pos.checked_add(1).ok_or_else(|| {
            LqParseError::new(
                LqParseErrorCode::SyntaxError,
                self.span,
                "structural offset overflow",
            )
        })?;
        Ok(())
    }

    fn parse_body(
        &mut self,
        depth: u32,
        stop_on_closing_brace: bool,
    ) -> Result<LqStructuralBlock, LqParseError> {
        let mut exprs: Vec<LqStructuralExpr> = Vec::new();
        let mut saw_pattern = false;
        let mut saw_non_pattern = false;
        loop {
            self.skip_ascii_whitespace()?;
            match self.bytes.get(self.pos) {
                None => break,
                Some(b'}') if stop_on_closing_brace => break,
                Some(_) => {}
            }
            if self.peek_keyword_where() {
                if !saw_pattern {
                    return Err(LqParseError::new(
                        LqParseErrorCode::SyntaxError,
                        self.span,
                        "`where` requires a preceding structural pattern",
                    ));
                }
                exprs.push(LqStructuralExpr::Where(self.parse_where_clause()?));
                saw_non_pattern = true;
                continue;
            }
            if self.peek_keyword_context("inside") {
                if !saw_pattern {
                    return Err(LqParseError::new(
                        LqParseErrorCode::SyntaxError,
                        self.span,
                        "`inside` requires a preceding structural pattern",
                    ));
                }
                exprs.push(LqStructuralExpr::Inside(Box::new(
                    self.parse_context_block(depth, "inside")?,
                )));
                saw_non_pattern = true;
                continue;
            }
            if self.peek_keyword_context("outside") {
                if !saw_pattern {
                    return Err(LqParseError::new(
                        LqParseErrorCode::SyntaxError,
                        self.span,
                        "`outside` requires a preceding structural pattern",
                    ));
                }
                exprs.push(LqStructuralExpr::Outside(Box::new(
                    self.parse_context_block(depth, "outside")?,
                )));
                saw_non_pattern = true;
                continue;
            }
            if saw_non_pattern {
                return Err(LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    self.span,
                    "structural pattern tokens cannot appear after where/inside/outside",
                ));
            }
            let nodes = self.parse_group_body(depth, true)?;
            if nodes.is_empty() {
                return Err(LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    self.span,
                    "empty structural pattern is not allowed",
                ));
            }
            exprs.push(LqStructuralExpr::Pattern(nodes));
            saw_pattern = true;
        }
        if exprs.is_empty() {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                self.span,
                "empty structural body is not allowed",
            ));
        }
        let nodes = match exprs.first() {
            Some(LqStructuralExpr::Pattern(nodes)) => nodes.clone(),
            _ => Vec::new(),
        };
        Ok(LqStructuralBlock {
            lang: None,
            nodes,
            exprs,
        })
    }

    fn parse_group_body(
        &mut self,
        depth: u32,
        stop_on_directive: bool,
    ) -> Result<Vec<LqStructuralNode>, LqParseError> {
        let max_depth = crate::limits::MAX_AST_DEPTH;
        if depth > max_depth {
            return Err(LqParseError::new(
                LqParseErrorCode::LimitExceededDepth,
                self.span,
                "structural nesting depth exceeds 32",
            ));
        }
        let mut out: Vec<LqStructuralNode> = Vec::new();
        let mut literal_buf: Vec<u8> = Vec::new();
        loop {
            if stop_on_directive && self.starts_expr_directive() {
                if !literal_buf.is_empty() {
                    out.push(LqStructuralNode::Literal(bytes_to_box(
                        &literal_buf,
                        self.span,
                    )?));
                    literal_buf.clear();
                }
                return Ok(out);
            }
            let Some(&b) = self.bytes.get(self.pos) else {
                if !literal_buf.is_empty() {
                    out.push(LqStructuralNode::Literal(bytes_to_box(
                        &literal_buf,
                        self.span,
                    )?));
                    literal_buf.clear();
                }
                return Ok(out);
            };
            match b {
                b'.' if self.peek_sequence(b"...") => {
                    if !literal_buf.is_empty() {
                        out.push(LqStructuralNode::Literal(bytes_to_box(
                            &literal_buf,
                            self.span,
                        )?));
                        literal_buf.clear();
                    }
                    self.bump()?;
                    self.bump()?;
                    self.bump()?;
                    out.push(LqStructuralNode::WildcardMany);
                }
                b'}' => {
                    if !literal_buf.is_empty() {
                        out.push(LqStructuralNode::Literal(bytes_to_box(
                            &literal_buf,
                            self.span,
                        )?));
                        literal_buf.clear();
                    }
                    return Ok(out);
                }
                b'{' => {
                    if !literal_buf.is_empty() {
                        out.push(LqStructuralNode::Literal(bytes_to_box(
                            &literal_buf,
                            self.span,
                        )?));
                        literal_buf.clear();
                    }
                    self.bump()?;
                    let next_depth = depth.checked_add(1).ok_or_else(|| {
                        LqParseError::new(
                            LqParseErrorCode::LimitExceededDepth,
                            self.span,
                            "structural depth counter overflow",
                        )
                    })?;
                    let inner = self.parse_group_body(next_depth, false)?;
                    if self.bytes.get(self.pos) != Some(&b'}') {
                        return Err(LqParseError::new(
                            LqParseErrorCode::SyntaxError,
                            self.span,
                            "unbalanced '{' — missing closing '}'",
                        ));
                    }
                    self.bump()?;
                    out.push(LqStructuralNode::Group(inner));
                }
                b'$' => {
                    if !literal_buf.is_empty() {
                        out.push(LqStructuralNode::Literal(bytes_to_box(
                            &literal_buf,
                            self.span,
                        )?));
                        literal_buf.clear();
                    }
                    out.push(self.parse_hole_node_dollar()?);
                }
                b':' if self.peek_alias() => {
                    if !literal_buf.is_empty() {
                        out.push(LqStructuralNode::Literal(bytes_to_box(
                            &literal_buf,
                            self.span,
                        )?));
                        literal_buf.clear();
                    }
                    out.push(self.parse_hole_node_alias()?);
                }
                _ => {
                    literal_buf.push(b);
                    self.bump()?;
                }
            }
        }
    }

    fn peek_alias(&self) -> bool {
        let Some(n) = self.pos.checked_add(1) else {
            return false;
        };
        self.bytes.get(n) == Some(&b'[')
    }

    fn peek_sequence(&self, needle: &[u8]) -> bool {
        self.bytes
            .get(self.pos..self.pos.saturating_add(needle.len()))
            == Some(needle)
    }

    fn skip_ascii_whitespace(&mut self) -> Result<(), LqParseError> {
        while self
            .bytes
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.bump()?;
        }
        Ok(())
    }

    fn consume_bytes(&mut self, count: usize) -> Result<(), LqParseError> {
        for _ in 0..count {
            self.bump()?;
        }
        Ok(())
    }

    fn starts_expr_directive(&self) -> bool {
        self.peek_keyword_where()
            || self.peek_keyword_context("inside")
            || self.peek_keyword_context("outside")
    }

    fn peek_keyword_where(&self) -> bool {
        self.peek_keyword("where", true)
    }

    fn peek_keyword_context(&self, keyword: &str) -> bool {
        self.peek_keyword(keyword, false)
    }

    fn peek_keyword_and(&self) -> bool {
        self.peek_keyword("AND", true)
    }

    fn peek_keyword(&self, keyword: &str, whitespace_only: bool) -> bool {
        let keyword = keyword.as_bytes();
        let Some(slice) = self
            .bytes
            .get(self.pos..self.pos.saturating_add(keyword.len()))
        else {
            return false;
        };
        if slice != keyword {
            return false;
        }
        match self.bytes.get(self.pos.saturating_add(keyword.len())) {
            Some(next) if next.is_ascii_whitespace() => true,
            Some(b'{') if !whitespace_only => true,
            _ => false,
        }
    }

    fn parse_context_block(
        &mut self,
        depth: u32,
        keyword: &str,
    ) -> Result<LqStructuralBlock, LqParseError> {
        self.consume_bytes(keyword.len())?;
        self.skip_ascii_whitespace()?;
        if self.bytes.get(self.pos) != Some(&b'{') {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                self.span,
                format!("`{keyword}` must be followed by `{{`"),
            ));
        }
        self.bump()?;
        let next_depth = depth.checked_add(1).ok_or_else(|| {
            LqParseError::new(
                LqParseErrorCode::LimitExceededDepth,
                self.span,
                "structural context depth counter overflow",
            )
        })?;
        let block = self.parse_body(next_depth, true)?;
        if self.bytes.get(self.pos) != Some(&b'}') {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                self.span,
                format!("`{keyword}` block missing closing `}}`"),
            ));
        }
        self.bump()?;
        Ok(block)
    }

    fn parse_where_clause(&mut self) -> Result<Vec<LqStructuralConstraint>, LqParseError> {
        self.consume_bytes("where".len())?;
        self.skip_ascii_whitespace()?;
        let mut constraints = Vec::new();
        loop {
            constraints.push(self.parse_structural_constraint()?);
            self.skip_ascii_whitespace()?;
            if !self.peek_keyword_and() {
                break;
            }
            self.consume_bytes("AND".len())?;
            self.skip_ascii_whitespace()?;
        }
        Ok(constraints)
    }

    fn parse_structural_constraint(&mut self) -> Result<LqStructuralConstraint, LqParseError> {
        let left = self.parse_hole_ref()?;
        self.skip_ascii_whitespace()?;
        if self.bytes.get(self.pos) != Some(&b'=')
            || self.bytes.get(self.pos.saturating_add(1)) != Some(&b'=')
        {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                self.span,
                "structural constraint requires `==`",
            ));
        }
        self.consume_bytes(2)?;
        self.skip_ascii_whitespace()?;
        let right = match self.bytes.get(self.pos) {
            Some(b'$') => LqStructuralConstraintOperand::Hole(self.parse_hole_ref()?),
            Some(b'"') => LqStructuralConstraintOperand::Phrase(self.parse_phrase_literal()?),
            Some(b'\'') => LqStructuralConstraintOperand::RawString(self.parse_raw_literal()?),
            Some(b'/') => LqStructuralConstraintOperand::Regex(self.parse_regex_literal()?),
            _ => {
                return Err(LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    self.span,
                    "structural constraint RHS must be a hole, phrase, raw string, or regex",
                ));
            }
        };
        Ok(LqStructuralConstraint { left, right })
    }

    fn parse_hole_ref(&mut self) -> Result<LqStructuralHoleRef, LqParseError> {
        let (multiplicity, name) = match self.bytes.get(self.pos) {
            Some(b'$') => {
                self.bump()?;
                self.parse_metavar_dollar()?
            }
            Some(b':') if self.peek_alias() => {
                self.bump()?;
                self.bump()?;
                if self
                    .bytes
                    .get(self.pos..)
                    .is_some_and(|tail| tail.starts_with(b"hole.type="))
                {
                    return Err(LqParseError::new(
                        LqParseErrorCode::SyntaxError,
                        self.span,
                        "typed structural holes are not supported on the current native route",
                    ));
                }
                let parsed = self.parse_metavar_until(b']')?;
                if self.bytes.get(self.pos) != Some(&b']') {
                    return Err(LqParseError::new(
                        LqParseErrorCode::SyntaxError,
                        self.span,
                        "metavariable ':[name]' missing closing ']'",
                    ));
                }
                self.bump()?;
                parsed
            }
            _ => {
                return Err(LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    self.span,
                    "structural hole reference must start with `$` or `:[`",
                ));
            }
        };
        Ok(LqStructuralHoleRef { name, multiplicity })
    }

    fn parse_phrase_literal(&mut self) -> Result<String, LqParseError> {
        self.bump()?;
        let mut out = String::new();
        loop {
            let Some(&b) = self.bytes.get(self.pos) else {
                return Err(LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    self.span,
                    "unterminated phrase literal in structural where",
                ));
            };
            match b {
                b'"' => {
                    self.bump()?;
                    return Ok(out);
                }
                b'\\' => {
                    self.bump()?;
                    let Some(&escaped) = self.bytes.get(self.pos) else {
                        return Err(LqParseError::new(
                            LqParseErrorCode::SyntaxError,
                            self.span,
                            "unterminated phrase escape in structural where",
                        ));
                    };
                    let decoded = match escaped {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        _ => {
                            return Err(LqParseError::new(
                                LqParseErrorCode::TokenInvalid,
                                self.span,
                                "unknown phrase escape in structural where",
                            ));
                        }
                    };
                    self.bump()?;
                    out.push(decoded);
                }
                _ => {
                    self.bump()?;
                    out.push(char::from(b));
                }
            }
        }
    }

    fn parse_raw_literal(&mut self) -> Result<String, LqParseError> {
        self.bump()?;
        let start = self.pos;
        while let Some(&b) = self.bytes.get(self.pos) {
            if b == b'\'' {
                let Some(bytes) = self.bytes.get(start..self.pos) else {
                    return Err(LqParseError::new(
                        LqParseErrorCode::SyntaxError,
                        self.span,
                        "internal: raw literal slice invalid",
                    ));
                };
                let text = match core::str::from_utf8(bytes) {
                    Ok(text) => text.to_owned(),
                    Err(_e) => {
                        return Err(LqParseError::new(
                            LqParseErrorCode::TokenInvalid,
                            self.span,
                            "invalid UTF-8 in structural raw string",
                        ));
                    }
                };
                self.bump()?;
                return Ok(text);
            }
            self.bump()?;
        }
        Err(LqParseError::new(
            LqParseErrorCode::SyntaxError,
            self.span,
            "unterminated raw string in structural where",
        ))
    }

    fn parse_regex_literal(&mut self) -> Result<String, LqParseError> {
        self.bump()?;
        let mut out = String::new();
        let mut escaped = false;
        loop {
            let Some(&byte) = self.bytes.get(self.pos) else {
                return Err(LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    self.span,
                    "unterminated regex literal in structural where",
                ));
            };
            self.bump()?;
            if escaped {
                out.push(char::from(byte));
                escaped = false;
                continue;
            }
            match byte {
                b'/' => return Ok(out),
                b'\\' => {
                    out.push('\\');
                    escaped = true;
                }
                _ => out.push(char::from(byte)),
            }
        }
    }

    fn parse_hole_node_dollar(&mut self) -> Result<LqStructuralNode, LqParseError> {
        self.bump()?;
        let (multiplicity, mv) = self.parse_metavar_dollar()?;
        Ok(LqStructuralNode::Hole {
            name: Some(mv),
            multiplicity,
        })
    }

    fn parse_hole_node_alias(&mut self) -> Result<LqStructuralNode, LqParseError> {
        self.bump()?;
        self.bump()?;
        if self
            .bytes
            .get(self.pos..)
            .is_some_and(|tail| tail.starts_with(b"hole.type="))
        {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                self.span,
                "typed structural holes are not supported on the current native route",
            ));
        }
        let (multiplicity, mv) = self.parse_metavar_until(b']')?;
        if self.bytes.get(self.pos) != Some(&b']') {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                self.span,
                "metavariable ':[name]' missing closing ']'",
            ));
        }
        self.bump()?;
        Ok(LqStructuralNode::Hole {
            name: Some(mv),
            multiplicity,
        })
    }

    fn parse_metavar_dollar(
        &mut self,
    ) -> Result<(LqStructuralHoleMultiplicity, LqMetaVar), LqParseError> {
        let multiplicity = if self.peek_sequence(b"...") {
            self.bump()?;
            self.bump()?;
            self.bump()?;
            LqStructuralHoleMultiplicity::Many
        } else {
            LqStructuralHoleMultiplicity::One
        };
        let start = self.pos;
        while let Some(&b) = self.bytes.get(self.pos) {
            let is_first = self.pos == start;
            let ok = if is_first {
                b.is_ascii_alphabetic() || b == b'_'
            } else {
                b.is_ascii_alphanumeric() || b == b'_'
            };
            if !ok {
                break;
            }
            self.bump()?;
        }
        if self.pos == start {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                self.span,
                "metavariable '$' missing name",
            ));
        }
        let Some(name_bytes) = self.bytes.get(start..self.pos) else {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                self.span,
                "internal: metavar slice invalid",
            ));
        };
        let name = match core::str::from_utf8(name_bytes) {
            Ok(s) => s.to_owned(),
            Err(_e) => {
                return Err(LqParseError::new(
                    LqParseErrorCode::TokenInvalid,
                    self.span,
                    "invalid UTF-8 in metavar name",
                ));
            }
        };
        Ok((multiplicity, LqMetaVar::new(name)))
    }

    fn parse_metavar_until(
        &mut self,
        terminator: u8,
    ) -> Result<(LqStructuralHoleMultiplicity, LqMetaVar), LqParseError> {
        let multiplicity = if self.peek_sequence(b"...") {
            self.bump()?;
            self.bump()?;
            self.bump()?;
            LqStructuralHoleMultiplicity::Many
        } else {
            LqStructuralHoleMultiplicity::One
        };
        let start = self.pos;
        while let Some(&b) = self.bytes.get(self.pos) {
            if b == terminator {
                break;
            }
            self.bump()?;
        }
        if self.pos == start {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                self.span,
                "metavariable ':[name]' missing name",
            ));
        }
        let Some(name_bytes) = self.bytes.get(start..self.pos) else {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                self.span,
                "internal: metavar slice invalid",
            ));
        };
        let name = match core::str::from_utf8(name_bytes) {
            Ok(s) => s.to_owned(),
            Err(_e) => {
                return Err(LqParseError::new(
                    LqParseErrorCode::TokenInvalid,
                    self.span,
                    "invalid UTF-8 in metavar name",
                ));
            }
        };
        Ok((multiplicity, LqMetaVar::new(name)))
    }
}

fn bytes_to_box(buf: &[u8], span: LqSpan) -> Result<Box<str>, LqParseError> {
    let s = match core::str::from_utf8(buf) {
        Ok(s) => s,
        Err(_e) => {
            return Err(LqParseError::new(
                LqParseErrorCode::TokenInvalid,
                span,
                "invalid UTF-8 in structural literal",
            ));
        }
    };
    Ok(s.to_owned().into_boxed_str())
}

#[cfg(test)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "test fixtures pin exact AST shapes and surface unexpected variants via assert!(false, …); fail-loud catch is the intent"
)]
mod tests {
    use super::parse;
    use crate::ast::{
        LqCountBound, LqDirective, LqExpr, LqFileScope, LqFilter, LqLeaf, LqPatternType, LqType,
    };
    use crate::errors::LqParseErrorCode;
    use crate::tokenizer::tokenize;

    fn parse_input(s: &str) -> crate::ast::LqNormalizedQuery {
        let toks = match tokenize(s) {
            Ok(t) => t,
            Err(e) => {
                assert!(false, "tokenize failed: {e}");
                return crate::ast::LqNormalizedQuery::empty(crate::errors::LqSpan::new(0, 0));
            }
        };
        match parse(&toks, s) {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "parse failed: {e}");
                crate::ast::LqNormalizedQuery::empty(crate::errors::LqSpan::new(0, 0))
            }
        }
    }

    fn parse_err(s: &str) -> LqParseErrorCode {
        let toks = match tokenize(s) {
            Ok(t) => t,
            Err(e) => return e.code,
        };
        match parse(&toks, s) {
            Ok(_) => {
                assert!(false, "parse({s:?}) unexpectedly succeeded");
                LqParseErrorCode::SyntaxError
            }
            Err(e) => e.code,
        }
    }

    #[test]
    fn single_keyword() {
        let q = parse_input("fooBar");
        assert_eq!(q.expr, LqExpr::Leaf(LqLeaf::Keyword("fooBar".to_owned())));
        assert!(q.filters.is_empty());
        assert!(q.directives.is_empty());
    }

    #[test]
    fn implicit_and_of_two_keywords() {
        let q = parse_input("tokio runtime");
        assert_eq!(
            q.expr,
            LqExpr::All(vec![
                LqExpr::Leaf(LqLeaf::Keyword("tokio".to_owned())),
                LqExpr::Leaf(LqLeaf::Keyword("runtime".to_owned())),
            ])
        );
    }

    #[test]
    fn explicit_or() {
        let q = parse_input("panic OR unwrap");
        assert_eq!(
            q.expr,
            LqExpr::Any(vec![
                LqExpr::Leaf(LqLeaf::Keyword("panic".to_owned())),
                LqExpr::Leaf(LqLeaf::Keyword("unwrap".to_owned())),
            ])
        );
    }

    #[test]
    fn not_binds_tighter_than_or() {
        let q = parse_input("foo OR NOT bar");
        assert_eq!(
            q.expr,
            LqExpr::Any(vec![
                LqExpr::Leaf(LqLeaf::Keyword("foo".to_owned())),
                LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Keyword("bar".to_owned())))),
            ])
        );
    }

    #[test]
    fn paren_group_overrides_default_precedence() {
        let q = parse_input("(panic OR unwrap) lang:rust");
        assert_eq!(
            q.expr,
            LqExpr::Any(vec![
                LqExpr::Leaf(LqLeaf::Keyword("panic".to_owned())),
                LqExpr::Leaf(LqLeaf::Keyword("unwrap".to_owned())),
            ])
        );
        assert_eq!(
            q.filters,
            vec![LqFilter::Lang {
                id: "rust".to_owned()
            }]
        );
    }

    #[test]
    fn dash_negation_at_atom_start() {
        let q = parse_input("Iterator -dyn");
        assert_eq!(
            q.expr,
            LqExpr::All(vec![
                LqExpr::Leaf(LqLeaf::Keyword("Iterator".to_owned())),
                LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Keyword("dyn".to_owned())))),
            ])
        );
    }

    #[test]
    fn repo_filter_with_at_rev() {
        let q = parse_input("repo:foo@main panic!");
        assert_eq!(
            q.filters,
            vec![LqFilter::Repo {
                pattern: "foo".to_owned(),
                revs: vec!["main".to_owned()],
            }]
        );
    }

    #[test]
    fn path_filter_maps_to_path_only_scope() {
        let q = parse_input("path:src/lib");
        assert_eq!(
            q.filters,
            vec![LqFilter::File {
                pattern: "src/lib".to_owned(),
                scope: LqFileScope::PathOnly,
            }]
        );
    }

    #[test]
    fn unknown_filter_errors() {
        assert_eq!(
            parse_err("not_a_filter:v foo"),
            LqParseErrorCode::UnknownFilter
        );
    }

    #[test]
    fn duplicate_type_filter_errors() {
        assert_eq!(
            parse_err("type:file type:diff foo"),
            LqParseErrorCode::InvalidFilterValue
        );
    }

    #[test]
    fn invalid_patterntype_errors() {
        assert_eq!(
            parse_err("patterntype:fuzzy foo"),
            LqParseErrorCode::InvalidPatternType
        );
    }

    #[test]
    fn count_all_and_bounded() {
        let q1 = parse_input("foo count:100");
        assert_eq!(q1.options.count, Some(LqCountBound::Bounded(100)));
        let q2 = parse_input("foo count:all");
        assert_eq!(q2.options.count, Some(LqCountBound::All));
    }

    #[test]
    fn timeout_lowered_into_options_ms() {
        let q1 = parse_input("timeout:0ms /.*/");
        assert_eq!(q1.options.timeout_ms, Some(0));
        let q2 = parse_input("timeout:5s /.*/");
        assert_eq!(q2.options.timeout_ms, Some(5_000));
    }

    #[test]
    fn invalid_timeout_errors() {
        assert_eq!(
            parse_err("timeout:soon /.*/"),
            LqParseErrorCode::InvalidFilterValue
        );
        assert_eq!(
            parse_err("timeout:5 /.*/"),
            LqParseErrorCode::InvalidFilterValue
        );
    }

    #[test]
    fn history_date_and_diff_filters_parse() {
        let q = parse_input(
            "type:diff diff.added:history diff.removed:removed diff.touched:touched before:1970-01-01T00:00:00.012Z after:1d since:2h until:1970-01-01",
        );
        assert_eq!(
            q.filters,
            vec![
                LqFilter::Type { kind: LqType::Diff },
                LqFilter::DiffAdded {
                    pattern: "history".to_owned(),
                },
                LqFilter::DiffRemoved {
                    pattern: "removed".to_owned(),
                },
                LqFilter::DiffTouched {
                    pattern: "touched".to_owned(),
                },
                LqFilter::Before {
                    timeref: "1970-01-01T00:00:00.012Z".to_owned(),
                },
                LqFilter::After {
                    timeref: "1d".to_owned(),
                },
                LqFilter::Since {
                    timeref: "2h".to_owned(),
                },
                LqFilter::Until {
                    timeref: "1970-01-01".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn qualified_since_filters_parse() {
        let q =
            parse_input("type:commit since.time:2024-01-01T00:00:00Z since.commit:refs/heads/main");
        assert_eq!(
            q.filters,
            vec![
                LqFilter::Type {
                    kind: LqType::Commit
                },
                LqFilter::Since {
                    timeref: "time:2024-01-01T00:00:00Z".to_owned(),
                },
                LqFilter::Since {
                    timeref: "commit:refs/heads/main".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn into_codeql_emits_directive() {
        let q = parse_input("Iterator into:codeql");
        assert_eq!(q.directives, vec![LqDirective::IntoCodeQl]);
    }

    #[test]
    fn unbalanced_paren_errors() {
        assert_eq!(parse_err("(foo"), LqParseErrorCode::SyntaxError);
    }

    #[test]
    fn trailing_operator_errors() {
        // `foo AND` → AND at the right edge, no right operand.
        assert_eq!(parse_err("foo AND"), LqParseErrorCode::SyntaxError);
    }

    #[test]
    fn deep_nesting_exceeds_depth() {
        // 33 open parens with no body collapse to an unmatched group long
        // before the 32-depth cap, so use a body-bearing nested form.
        let mut s = String::new();
        for _ in 0..40 {
            s.push('(');
        }
        s.push('x');
        for _ in 0..40 {
            s.push(')');
        }
        let code = parse_err(&s);
        assert_eq!(code, LqParseErrorCode::LimitExceededDepth);
    }

    #[test]
    fn or_fanout_exceeded() {
        // 70 OR-joined leaves under one node.
        let mut s = String::from("a0");
        for i in 1..70 {
            s.push_str(" OR a");
            s.push_str(&i.to_string());
        }
        assert_eq!(parse_err(&s), LqParseErrorCode::LimitExceededFanout);
    }

    #[test]
    fn patterntype_propagates_to_options() {
        let q = parse_input("patterntype:regexp foo");
        assert_eq!(q.options.pattern_type, LqPatternType::Regexp);
    }

    #[test]
    fn type_filter_accepted() {
        let q = parse_input("type:file foo");
        assert_eq!(q.filters, vec![LqFilter::Type { kind: LqType::File }]);
    }

    // ---- Predicate sub-parser tests (Step 1) ----

    #[test]
    fn predicate_repo_has_file_with_filter_arg() {
        let q = parse_input("repo:has.file(path:src)");
        match q.expr {
            LqExpr::Leaf(LqLeaf::Predicate { name, args }) => {
                assert_eq!(name, "repo.has.file");
                assert_eq!(
                    args,
                    vec![crate::ast::LqPredicateArg::Filter {
                        name: "path".to_owned(),
                        value: "src".to_owned(),
                    }]
                );
            }
            other => {
                assert!(false, "expected Predicate leaf, got {other:?}");
            }
        }
    }

    #[test]
    fn predicate_top_level_file_contains_with_raw_arg() {
        let q = parse_input("file.contains('oo_ba')");
        match q.expr {
            LqExpr::Leaf(LqLeaf::Predicate { name, args }) => {
                assert_eq!(name, "file.contains");
                assert_eq!(
                    args,
                    vec![crate::ast::LqPredicateArg::RawString("oo_ba".to_owned())]
                );
            }
            other => {
                assert!(false, "expected Predicate leaf, got {other:?}");
            }
        }
    }

    #[test]
    fn predicate_repo_contains_content_with_phrase_arg() {
        let q = parse_input("repo:contains.content(\"TODO\")");
        match q.expr {
            LqExpr::Leaf(LqLeaf::Predicate { name, args }) => {
                assert_eq!(name, "repo.contains.content");
                assert_eq!(
                    args,
                    vec![crate::ast::LqPredicateArg::Phrase("TODO".to_owned())]
                );
            }
            other => {
                assert!(false, "expected Predicate leaf, got {other:?}");
            }
        }
    }

    #[test]
    fn predicate_dotted_multi_segment_name() {
        let q = parse_input("repo:contains.commit.after(2024)");
        match q.expr {
            LqExpr::Leaf(LqLeaf::Predicate { name, args }) => {
                assert_eq!(name, "repo.contains.commit.after");
                assert_eq!(args, vec![crate::ast::LqPredicateArg::Number(2024)]);
            }
            other => {
                assert!(false, "expected Predicate leaf, got {other:?}");
            }
        }
    }

    #[test]
    fn predicate_unknown_name_accepts_at_parse() {
        // Unknown names parse cleanly — semantic rejection is planner-scope.
        let q = parse_input("repo:made.up.predicate(x)");
        match q.expr {
            LqExpr::Leaf(LqLeaf::Predicate { name, .. }) => {
                assert_eq!(name, "repo.made.up.predicate");
            }
            other => {
                assert!(false, "expected Predicate leaf, got {other:?}");
            }
        }
    }

    #[test]
    fn predicate_unclosed_paren_errors() {
        // Tokenizer surfaces the missing `)` as SyntaxError.
        assert_eq!(parse_err("repo:has.file(x"), LqParseErrorCode::SyntaxError);
    }

    #[test]
    fn predicate_multiple_args_split_on_comma() {
        let q = parse_input("repo:has.file(path:src, name:lib)");
        match q.expr {
            LqExpr::Leaf(LqLeaf::Predicate { args, .. }) => {
                assert_eq!(args.len(), 2);
            }
            other => {
                assert!(false, "expected Predicate leaf, got {other:?}");
            }
        }
    }

    // ---- Structural inner-parse tests (Step 2) ----

    #[test]
    fn structural_block_parses_pure_literal_body() {
        let q = parse_input("match { hello }");
        match q.expr {
            LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => {
                assert!(block.lang.is_none());
                assert_eq!(block.exprs.len(), 1);
                let Some(crate::ast::LqStructuralExpr::Pattern(nodes)) = block.exprs.first() else {
                    assert!(false, "expected first expr to be structural pattern");
                    return;
                };
                assert_eq!(nodes.len(), 1);
                match nodes.first() {
                    Some(crate::ast::LqStructuralNode::Literal(s)) => {
                        assert_eq!(s.as_ref(), "hello ");
                    }
                    other => {
                        assert!(false, "expected Literal, got {other:?}");
                    }
                }
            }
            other => {
                assert!(false, "expected StructuralBlock leaf, got {other:?}");
            }
        }
    }

    #[test]
    fn structural_block_parses_metavar_dollar_form() {
        let q = parse_input("match { fn $X() }");
        match q.expr {
            LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => {
                let Some(crate::ast::LqStructuralExpr::Pattern(nodes)) = block.exprs.first() else {
                    assert!(false, "expected first expr to be structural pattern");
                    return;
                };
                let metavars: Vec<&crate::ast::LqStructuralNode> = block
                    .exprs
                    .iter()
                    .flat_map(|expr| match expr {
                        crate::ast::LqStructuralExpr::Pattern(nodes) => nodes.iter().collect(),
                        _ => Vec::new(),
                    })
                    .collect();
                assert_eq!(nodes.len(), 3);
                let captures: Vec<&crate::ast::LqStructuralNode> = metavars
                    .iter()
                    .copied()
                    .filter(|n| {
                        matches!(
                            n,
                            crate::ast::LqStructuralNode::Hole {
                                name: Some(_),
                                multiplicity: crate::ast::LqStructuralHoleMultiplicity::One,
                            }
                        )
                    })
                    .collect();
                assert_eq!(captures.len(), 1);
                if let Some(crate::ast::LqStructuralNode::Hole {
                    name: Some(m),
                    multiplicity: crate::ast::LqStructuralHoleMultiplicity::One,
                }) = captures.first().copied()
                {
                    assert_eq!(m.as_str(), "X");
                } else {
                    assert!(false, "expected $X metavar");
                }
            }
            other => {
                assert!(false, "expected StructuralBlock leaf, got {other:?}");
            }
        }
    }

    #[test]
    fn structural_block_parses_alias_form() {
        let q = parse_input("match { fn :[name]() }");
        match q.expr {
            LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => {
                let captures: Vec<&crate::ast::LqStructuralNode> = block
                    .exprs
                    .iter()
                    .flat_map(|expr| match expr {
                        crate::ast::LqStructuralExpr::Pattern(nodes) => nodes.iter().collect(),
                        _ => Vec::new(),
                    })
                    .filter(|n| {
                        matches!(
                            n,
                            crate::ast::LqStructuralNode::Hole {
                                name: Some(_),
                                multiplicity: crate::ast::LqStructuralHoleMultiplicity::One,
                            }
                        )
                    })
                    .collect();
                assert_eq!(captures.len(), 1);
                if let Some(crate::ast::LqStructuralNode::Hole {
                    name: Some(m),
                    multiplicity: crate::ast::LqStructuralHoleMultiplicity::One,
                }) = captures.first().copied()
                {
                    assert_eq!(m.as_str(), "name");
                } else {
                    assert!(false, "expected :[name] metavar");
                }
            }
            other => {
                assert!(false, "expected StructuralBlock leaf, got {other:?}");
            }
        }
    }

    #[test]
    fn structural_block_parses_typed_hole_suffix_form() {
        let q = parse_input("match { fn :[name.expr]() }");
        match q.expr {
            LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => {
                let captures: Vec<&crate::ast::LqStructuralNode> = block
                    .exprs
                    .iter()
                    .flat_map(|expr| match expr {
                        crate::ast::LqStructuralExpr::Pattern(nodes) => nodes.iter().collect(),
                        _ => Vec::new(),
                    })
                    .filter(|n| {
                        matches!(
                            n,
                            crate::ast::LqStructuralNode::Hole {
                                name: Some(_),
                                multiplicity: crate::ast::LqStructuralHoleMultiplicity::One,
                            }
                        )
                    })
                    .collect();
                assert_eq!(captures.len(), 1);
                if let Some(crate::ast::LqStructuralNode::Hole {
                    name: Some(m),
                    multiplicity: crate::ast::LqStructuralHoleMultiplicity::One,
                }) = captures.first().copied()
                {
                    assert_eq!(m.as_str(), "name.expr");
                } else {
                    assert!(false, "expected :[name.expr] typed-hole alias");
                }
            }
            other => {
                assert!(false, "expected StructuralBlock leaf, got {other:?}");
            }
        }
    }

    #[test]
    fn structural_block_parses_nested_group() {
        let q = parse_input("match { fn $X() { $body } }");
        match q.expr {
            LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => {
                let Some(crate::ast::LqStructuralExpr::Pattern(nodes)) = block.exprs.first() else {
                    assert!(false, "expected first expr to be structural pattern");
                    return;
                };
                let has_group = nodes
                    .iter()
                    .any(|n| matches!(n, crate::ast::LqStructuralNode::Group(_)));
                assert!(has_group, "expected at least one nested Group");
            }
            other => {
                assert!(false, "expected StructuralBlock leaf, got {other:?}");
            }
        }
    }

    #[test]
    fn structural_block_parses_variadic_holes_and_wildcards() {
        let q = parse_input("match { fn $...args(...) { ... } }");
        match q.expr {
            LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => {
                let Some(crate::ast::LqStructuralExpr::Pattern(nodes)) = block.exprs.first() else {
                    assert!(false, "expected first expr to be structural pattern");
                    return;
                };
                assert!(nodes.iter().any(|node| matches!(
                    node,
                    crate::ast::LqStructuralNode::Hole {
                        name: Some(name),
                        multiplicity: crate::ast::LqStructuralHoleMultiplicity::Many,
                    } if name.as_str() == "args"
                )));
                assert!(
                    nodes
                        .iter()
                        .any(|node| matches!(node, crate::ast::LqStructuralNode::WildcardMany))
                );
            }
            other => assert!(false, "expected StructuralBlock leaf, got {other:?}"),
        }
    }

    #[test]
    fn structural_block_parses_where_inside_outside_exprs() {
        let q = parse_input(
            "match { fn $X() where $X == \"name\" AND :[X] == 'name' AND $X == /na.*/ inside { impl $T { ... } } outside { trait $T { ... } } }",
        );
        match q.expr {
            LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => {
                assert_eq!(block.exprs.len(), 4);
                assert!(matches!(
                    block.exprs.first(),
                    Some(crate::ast::LqStructuralExpr::Pattern(_))
                ));
                match block.exprs.get(1) {
                    Some(crate::ast::LqStructuralExpr::Where(constraints)) => {
                        assert_eq!(constraints.len(), 3);
                        let Some(first) = constraints.first() else {
                            assert!(false, "missing first structural constraint");
                            return;
                        };
                        let Some(second) = constraints.get(1) else {
                            assert!(false, "missing second structural constraint");
                            return;
                        };
                        let Some(third) = constraints.get(2) else {
                            assert!(false, "missing third structural constraint");
                            return;
                        };
                        assert_eq!(first.left.name.as_str(), "X");
                        assert_eq!(second.left.name.as_str(), "X");
                        assert_eq!(third.left.name.as_str(), "X");
                        assert!(matches!(
                            third.right,
                            crate::ast::LqStructuralConstraintOperand::Regex(ref pattern)
                                if pattern == "na.*"
                        ));
                    }
                    other => assert!(false, "expected where expr, got {other:?}"),
                }
                assert!(matches!(
                    block.exprs.get(2),
                    Some(crate::ast::LqStructuralExpr::Inside(_))
                ));
                assert!(matches!(
                    block.exprs.get(3),
                    Some(crate::ast::LqStructuralExpr::Outside(_))
                ));
            }
            other => assert!(false, "expected StructuralBlock leaf, got {other:?}"),
        }
    }

    #[test]
    fn structural_block_rejects_typed_hole_alias() {
        assert_eq!(
            parse_err("match { fn :[hole.type=ident]() }"),
            LqParseErrorCode::SyntaxError
        );
    }
}
