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
    LqNormalizedQuery, LqOptions, LqPatternType, LqSelect, LqType, LqVisibility, LqYesNoOnly,
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
                    let expected_child_count =
                        children.len().saturating_add(1);
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
            | LqTokenKind::KeywordOrFilterName(_) => {
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
            LqTokenKind::Regex(s) => Ok(Some(LqExpr::Leaf(LqLeaf::Regex(s)))),
            LqTokenKind::StructuralBlock(s) => Ok(Some(LqExpr::Leaf(LqLeaf::StructuralBlock(s)))),
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
                    let LqTokenKind::ColonValue(val) = value.kind else {
                        return Err(LqParseError::new(
                            LqParseErrorCode::InvalidFilterValue,
                            value.span,
                            "missing filter value",
                        ));
                    };
                    self.absorb_filter_or_option(&name, &val, head.span)?;
                    Ok(None)
                } else {
                    Ok(Some(LqExpr::Leaf(LqLeaf::Keyword(name))))
                }
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
                self.filters.push(LqFilter::Repo {
                    pattern: pat,
                    revs,
                });
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

#[cfg(test)]
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
        assert_eq!(parse_err("not_a_filter:v foo"), LqParseErrorCode::UnknownFilter);
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
}
