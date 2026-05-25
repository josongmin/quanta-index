//! LQ tokenizer — pure lex over a `&str` input.
//!
//! The lexer is byte-indexed against the original input; every token carries
//! an [`LqSpan`] with `[start, end)` byte offsets so the parser and error
//! envelopes can attribute every AST node to a source range.
//!
//! Tokenizer recognizes a subset of the dsl.md §1.7 token kinds large enough
//! to drive the canonical parse → normalize → hash pipeline that PRE-CONF
//! consumes; structural sub-grammar, predicate calls, and bridge directives
//! beyond `into:codeql` are deferred to a follow-up ticket (see PRE-NORM
//! ticket §4.1 entries for `predicates.rs` and `structural.rs`).

use crate::errors::{LqParseError, LqParseErrorCode, LqSpan};
use crate::limits::{MAX_INPUT_BYTES, MAX_STRUCTURAL_NODES};

/// One token emitted by [`tokenize`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LqToken {
    pub kind: LqTokenKind,
    pub span: LqSpan,
}

/// Token kind.
///
/// `KeywordOrFilterName` is the lex-time class for bare identifiers; the
/// parser disambiguates `<id>:<value>` (filter) from `<id>` (keyword leaf)
/// by lookahead for the `:` token, so the tokenizer does not pre-resolve it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LqTokenKind {
    /// Bare identifier — either a keyword leaf or a filter name.
    KeywordOrFilterName(String),
    /// `"…"` phrase, escape-decoded.
    Phrase(String),
    /// `'…'` raw string, with backslashes preserved literally.
    RawString(String),
    /// `/…/` regex source body.
    Regex(String),
    /// `match { … }` body, surrounding braces excluded.
    StructuralBlock(String),
    /// Filter value text following a `:` separator.
    ColonValue(String),
    /// Predicate value following a `:` separator. `dotted` is the dotted
    /// suffix (e.g. `has.file` for `repo:has.file(...)`); `args_raw` is
    /// the parenthesised argument list with surrounding `(` / `)` removed
    /// and trailing/leading whitespace preserved verbatim. The parser
    /// joins `dotted` with the preceding filter-name token to construct
    /// the canonical predicate name.
    Predicate {
        /// Dotted predicate suffix after `:`.
        dotted: String,
        /// Raw argument body verbatim (no outer parens).
        args_raw: String,
    },
    /// `:` separator.
    Colon,
    /// Boolean `AND` keyword.
    And,
    /// Boolean `OR` keyword.
    Or,
    /// Boolean `NOT` keyword.
    Not,
    /// `-` dash-negation at atom-start position.
    Dash,
    /// `(`.
    LParen,
    /// `)`.
    RParen,
    /// End-of-input marker.
    Eof,
}

/// Tokenize a raw input string. Enforces the 16 KiB input-byte cap before
/// scanning to avoid pathological lex cost on adversarial input.
pub fn tokenize(input: &str) -> Result<Vec<LqToken>, LqParseError> {
    if input.len() > MAX_INPUT_BYTES {
        let span_at = u32_from_usize(MAX_INPUT_BYTES)?;
        return Err(LqParseError::new(
            LqParseErrorCode::LimitExceededBytes,
            LqSpan::eof(span_at),
            "input exceeds 16 KiB cap",
        ));
    }
    let mut lex = Lexer::new(input);
    let mut out: Vec<LqToken> = Vec::new();
    loop {
        let ws_skipped = lex.skip_ws();
        let Some(byte) = lex.peek_byte() else {
            break;
        };
        let start = lex.pos;
        match byte {
            b'(' => {
                lex.advance_one();
                out.push(LqToken {
                    kind: LqTokenKind::LParen,
                    span: span_from(start, lex.pos)?,
                });
            }
            b')' => {
                lex.advance_one();
                out.push(LqToken {
                    kind: LqTokenKind::RParen,
                    span: span_from(start, lex.pos)?,
                });
            }
            b'-' => {
                if is_atom_start(out.last().map(|t| &t.kind), ws_skipped) {
                    lex.advance_one();
                    out.push(LqToken {
                        kind: LqTokenKind::Dash,
                        span: span_from(start, lex.pos)?,
                    });
                } else {
                    let tok = lex.read_identifier_or_filter(start)?;
                    out.push(tok);
                }
            }
            b'"' => {
                let tok = lex.read_phrase(start)?;
                out.push(tok);
            }
            b'\'' => {
                let tok = lex.read_raw_string(start)?;
                out.push(tok);
            }
            b'/' => {
                let tok = lex.read_regex(start)?;
                out.push(tok);
            }
            b':' => {
                lex.advance_one();
                out.push(LqToken {
                    kind: LqTokenKind::Colon,
                    span: span_from(start, lex.pos)?,
                });
            }
            _ => {
                let after_colon = matches!(out.last().map(|t| &t.kind), Some(LqTokenKind::Colon));
                if after_colon {
                    if lex.lookahead_is_predicate() {
                        let tok = lex.read_predicate(start)?;
                        out.push(tok);
                    } else {
                        let tok = lex.read_filter_value(start)?;
                        out.push(tok);
                    }
                } else if lex.starts_with(b"match") && lex.lookahead_after_match_is_brace() {
                    let tok = lex.read_structural_block(start)?;
                    out.push(tok);
                } else {
                    let tok = lex.read_identifier_or_filter(start)?;
                    out.push(tok);
                }
            }
        }
    }
    out.push(LqToken {
        kind: LqTokenKind::Eof,
        span: LqSpan::eof(u32_from_usize(input.len())?),
    });
    Ok(out)
}

/// Convert a byte offset within the 16 KiB-bounded input to `u32`.
///
/// Returns a `LimitExceededBytes` error if (impossibly, given the upfront
/// cap) the value would not fit; this keeps the lexer fail-closed without
/// silent truncation.
fn u32_from_usize(n: usize) -> Result<u32, LqParseError> {
    u32::try_from(n).map_err(|_e| {
        LqParseError::new(
            LqParseErrorCode::LimitExceededBytes,
            LqSpan::new(0, 0),
            "byte offset overflows u32",
        )
    })
}

fn span_from(start: usize, end: usize) -> Result<LqSpan, LqParseError> {
    Ok(LqSpan::new(u32_from_usize(start)?, u32_from_usize(end)?))
}

fn is_atom_start(prev: Option<&LqTokenKind>, ws_skipped: bool) -> bool {
    match prev {
        None => true,
        Some(
            LqTokenKind::And
            | LqTokenKind::Or
            | LqTokenKind::Not
            | LqTokenKind::LParen
            | LqTokenKind::Colon,
        ) => true,
        // Atom-start after a non-colon-non-boolean token requires a
        // whitespace separator: in `(a -b)` the `-` is at atom-start
        // because whitespace divides it from `a`; in `kebab-case` the
        // `-` is intra-identifier and was consumed by the identifier
        // reader, so the per-byte arm never sees it.
        _ => ws_skipped,
    }
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            src: input.as_bytes(),
            pos: 0,
        }
    }

    fn peek_byte(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn advance_one(&mut self) {
        self.pos = self.pos.saturating_add(1).min(self.src.len());
    }

    fn advance_by(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n).min(self.src.len());
    }

    fn skip_ws(&mut self) -> bool {
        let was = self.pos;
        while let Some(&b) = self.src.get(self.pos) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos = self.pos.saturating_add(1);
            } else {
                break;
            }
        }
        self.pos != was
    }

    fn starts_with(&self, needle: &[u8]) -> bool {
        let end = self.pos.saturating_add(needle.len());
        if end > self.src.len() {
            return false;
        }
        self.src.get(self.pos..end) == Some(needle)
    }

    /// Returns true if the cursor position starts a dotted predicate of the
    /// shape `<ident>(.<ident>)+(`. The `.<ident>` makes a predicate
    /// distinguishable from a function call on a plain identifier (which
    /// PRE-NORM does not accept in v1 — those are still lexed as
    /// `Ident + LParen`).
    fn lookahead_is_predicate(&self) -> bool {
        let mut p = self.pos;
        // First ident segment.
        let after_first = match self.scan_ident(p) {
            Some(end) if end > p => end,
            _ => return false,
        };
        p = after_first;
        // Must have at least one `.<ident>` follow.
        let Some(&first_sep) = self.src.get(p) else {
            return false;
        };
        if first_sep != b'.' {
            return false;
        }
        let mut saw_dot_ident = false;
        while let Some(&b) = self.src.get(p) {
            if b != b'.' {
                break;
            }
            let Some(dot_pos) = p.checked_add(1) else {
                return false;
            };
            let after_seg = match self.scan_ident(dot_pos) {
                Some(end) if end > dot_pos => end,
                _ => return false,
            };
            saw_dot_ident = true;
            p = after_seg;
        }
        if !saw_dot_ident {
            return false;
        }
        self.src.get(p) == Some(&b'(')
    }

    /// Scan an ASCII identifier `[A-Za-z_][A-Za-z0-9_-]*` starting at `from`.
    /// Returns the byte offset after the identifier, or `None` if the first
    /// byte isn't an identifier head.
    fn scan_ident(&self, from: usize) -> Option<usize> {
        let mut p = from;
        let &head = self.src.get(p)?;
        if !(head.is_ascii_alphabetic() || head == b'_') {
            return None;
        }
        p = p.checked_add(1)?;
        while let Some(&b) = self.src.get(p) {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
                let next = p.checked_add(1)?;
                p = next;
            } else {
                break;
            }
        }
        Some(p)
    }

    /// Read a predicate token of the shape `<ident>(.<ident>)+(args)`.
    /// Caller must have verified [`lookahead_is_predicate`] is true.
    fn read_predicate(&mut self, start: usize) -> Result<LqToken, LqParseError> {
        // Capture the dotted name byte range, then the args body.
        let dotted_start = self.pos;
        // We know the shape is valid; scan the dotted name.
        let mut last_seg_end = match self.scan_ident(self.pos) {
            Some(end) if end > self.pos => end,
            _ => {
                return Err(LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    span_from(start, self.pos)?,
                    "internal: predicate head missing",
                ));
            }
        };
        self.pos = last_seg_end;
        while let Some(&b) = self.src.get(self.pos) {
            if b != b'.' {
                break;
            }
            let dot_pos = self.pos.checked_add(1).ok_or_else(|| {
                LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    LqSpan::new(0, 0),
                    "internal: predicate offset overflow",
                )
            })?;
            last_seg_end = match self.scan_ident(dot_pos) {
                Some(end) if end > dot_pos => end,
                _ => {
                    return Err(LqParseError::new(
                        LqParseErrorCode::SyntaxError,
                        span_from(start, self.pos)?,
                        "predicate dotted segment missing identifier",
                    ));
                }
            };
            self.pos = last_seg_end;
        }
        let dotted_end = last_seg_end;
        let Some(dotted_bytes) = self.src.get(dotted_start..dotted_end) else {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                span_from(start, self.pos)?,
                "internal: predicate dotted slice invalid",
            ));
        };
        let dotted = match core::str::from_utf8(dotted_bytes) {
            Ok(s) => s.to_owned(),
            Err(_e) => {
                return Err(LqParseError::new(
                    LqParseErrorCode::TokenInvalid,
                    span_from(start, self.pos)?,
                    "invalid UTF-8 in predicate name",
                ));
            }
        };
        // Now consume `(`, body, `)`.
        if self.src.get(self.pos) != Some(&b'(') {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                span_from(start, self.pos)?,
                "predicate missing '('",
            ));
        }
        self.advance_one();
        let args_start = self.pos;
        let mut depth: u32 = 1;
        loop {
            let Some(&b) = self.src.get(self.pos) else {
                return Err(LqParseError::new(
                    LqParseErrorCode::SyntaxError,
                    span_from(start, self.pos)?,
                    "predicate missing ')'",
                ));
            };
            match b {
                b'(' => {
                    depth = depth.saturating_add(1);
                    self.advance_one();
                }
                b')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        let args_end = self.pos;
                        self.advance_one();
                        let Some(args_bytes) = self.src.get(args_start..args_end) else {
                            return Err(LqParseError::new(
                                LqParseErrorCode::SyntaxError,
                                span_from(start, self.pos)?,
                                "internal: predicate args slice invalid",
                            ));
                        };
                        let args_raw = match core::str::from_utf8(args_bytes) {
                            Ok(s) => s.to_owned(),
                            Err(_e) => {
                                return Err(LqParseError::new(
                                    LqParseErrorCode::TokenInvalid,
                                    span_from(start, self.pos)?,
                                    "invalid UTF-8 in predicate args",
                                ));
                            }
                        };
                        return Ok(LqToken {
                            kind: LqTokenKind::Predicate { dotted, args_raw },
                            span: span_from(start, self.pos)?,
                        });
                    }
                    self.advance_one();
                }
                b'"' => {
                    // Skip quoted phrase inside args; balance check stays
                    // intact regardless of `(`/`)` inside the phrase body.
                    self.advance_one();
                    while let Some(&inner) = self.src.get(self.pos) {
                        if inner == b'\\' {
                            self.advance_one();
                            self.advance_one();
                            continue;
                        }
                        if inner == b'"' {
                            self.advance_one();
                            break;
                        }
                        self.advance_one();
                    }
                }
                b'\'' => {
                    self.advance_one();
                    while let Some(&inner) = self.src.get(self.pos) {
                        if inner == b'\'' {
                            self.advance_one();
                            break;
                        }
                        self.advance_one();
                    }
                }
                _ => {
                    self.advance_one();
                }
            }
        }
    }

    fn lookahead_after_match_is_brace(&self) -> bool {
        let mut p = self.pos.saturating_add(5);
        while let Some(&b) = self.src.get(p) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                p = p.saturating_add(1);
            } else {
                break;
            }
        }
        self.src.get(p) == Some(&b'{')
    }

    fn read_phrase(&mut self, start: usize) -> Result<LqToken, LqParseError> {
        self.advance_one();
        let mut buf = String::new();
        loop {
            let Some(&b) = self.src.get(self.pos) else {
                return Err(LqParseError::new(
                    LqParseErrorCode::UnclosedQuote,
                    span_from(start, self.pos)?,
                    "unterminated phrase",
                ));
            };
            match b {
                b'"' => {
                    self.advance_one();
                    return Ok(LqToken {
                        kind: LqTokenKind::Phrase(buf),
                        span: span_from(start, self.pos)?,
                    });
                }
                b'\\' => {
                    self.advance_one();
                    let Some(&esc) = self.src.get(self.pos) else {
                        return Err(LqParseError::new(
                            LqParseErrorCode::TokenInvalid,
                            span_from(start, self.pos)?,
                            "trailing backslash in phrase",
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
                                span_from(self.pos.saturating_sub(1), self.pos.saturating_add(1))?,
                                "unknown phrase escape",
                            ));
                        }
                    };
                    buf.push(mapped);
                    self.advance_one();
                }
                _ => {
                    let Some(ch) = decode_utf8(self.src, self.pos) else {
                        return Err(LqParseError::new(
                            LqParseErrorCode::TokenInvalid,
                            span_from(start, self.pos)?,
                            "invalid UTF-8 in phrase",
                        ));
                    };
                    buf.push(ch);
                    self.advance_by(ch.len_utf8());
                }
            }
        }
    }

    fn read_raw_string(&mut self, start: usize) -> Result<LqToken, LqParseError> {
        self.advance_one();
        let mut buf = String::new();
        loop {
            let Some(&b) = self.src.get(self.pos) else {
                return Err(LqParseError::new(
                    LqParseErrorCode::UnclosedQuote,
                    span_from(start, self.pos)?,
                    "unterminated raw string",
                ));
            };
            if b == b'\'' {
                self.advance_one();
                return Ok(LqToken {
                    kind: LqTokenKind::RawString(buf),
                    span: span_from(start, self.pos)?,
                });
            }
            let Some(ch) = decode_utf8(self.src, self.pos) else {
                return Err(LqParseError::new(
                    LqParseErrorCode::TokenInvalid,
                    span_from(start, self.pos)?,
                    "invalid UTF-8 in raw string",
                ));
            };
            buf.push(ch);
            self.advance_by(ch.len_utf8());
        }
    }

    fn read_regex(&mut self, start: usize) -> Result<LqToken, LqParseError> {
        self.advance_one();
        let mut buf = String::new();
        loop {
            let Some(&b) = self.src.get(self.pos) else {
                return Err(LqParseError::new(
                    LqParseErrorCode::UnclosedQuote,
                    span_from(start, self.pos)?,
                    "unterminated regex",
                ));
            };
            if b == b'/' {
                self.advance_one();
                return Ok(LqToken {
                    kind: LqTokenKind::Regex(buf),
                    span: span_from(start, self.pos)?,
                });
            }
            if b == b'\\' {
                buf.push('\\');
                self.advance_one();
                let Some(&nxt) = self.src.get(self.pos) else {
                    return Err(LqParseError::new(
                        LqParseErrorCode::TokenInvalid,
                        span_from(start, self.pos)?,
                        "trailing backslash in regex",
                    ));
                };
                buf.push(char::from(nxt));
                self.advance_one();
                continue;
            }
            let Some(ch) = decode_utf8(self.src, self.pos) else {
                return Err(LqParseError::new(
                    LqParseErrorCode::TokenInvalid,
                    span_from(start, self.pos)?,
                    "invalid UTF-8 in regex",
                ));
            };
            buf.push(ch);
            self.advance_by(ch.len_utf8());
        }
    }

    fn read_structural_block(&mut self, start: usize) -> Result<LqToken, LqParseError> {
        // We've verified `match` then `{` follows. Consume `match`, ws, `{`.
        self.advance_by(5);
        // skip_ws returns bool indicating ws was consumed; discarding it
        // here is intentional — we only need the side effect.
        let _ws_consumed: bool = self.skip_ws();
        self.advance_one();
        let body_start = self.pos;
        let mut depth: u32 = 1;
        let mut node_count: u32 = 0;
        while let Some(&b) = self.src.get(self.pos) {
            match b {
                b'{' => {
                    depth = depth.saturating_add(1);
                    node_count = node_count.saturating_add(1);
                    self.advance_one();
                }
                b'}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        let body_end = self.pos;
                        self.advance_one();
                        if node_count > MAX_STRUCTURAL_NODES {
                            return Err(LqParseError::new(
                                LqParseErrorCode::LimitExceededStructural,
                                span_from(start, self.pos)?,
                                "structural pattern node count exceeds 256",
                            ));
                        }
                        let Some(body_slice) = self.src.get(body_start..body_end) else {
                            return Err(LqParseError::new(
                                LqParseErrorCode::SyntaxError,
                                span_from(start, self.pos)?,
                                "internal: structural body range invalid",
                            ));
                        };
                        let body = match core::str::from_utf8(body_slice) {
                            Ok(s) => s,
                            Err(_e) => {
                                return Err(LqParseError::new(
                                    LqParseErrorCode::TokenInvalid,
                                    span_from(start, self.pos)?,
                                    "invalid UTF-8 in structural body",
                                ));
                            }
                        };
                        return Ok(LqToken {
                            kind: LqTokenKind::StructuralBlock(body.to_owned()),
                            span: span_from(start, self.pos)?,
                        });
                    }
                    self.advance_one();
                }
                _ => {
                    if b == b'$'
                        || (b == b':' && self.src.get(self.pos.saturating_add(1)) == Some(&b'['))
                    {
                        node_count = node_count.saturating_add(1);
                    }
                    self.advance_one();
                }
            }
        }
        Err(LqParseError::new(
            LqParseErrorCode::SyntaxError,
            span_from(start, self.pos)?,
            "unterminated match { block",
        ))
    }

    fn read_identifier_or_filter(&mut self, start: usize) -> Result<LqToken, LqParseError> {
        while let Some(&b) = self.src.get(self.pos) {
            if matches!(
                b,
                b' ' | b'\t' | b'\n' | b'\r' | b'(' | b')' | b':' | b'"' | b'\'' | b'/'
            ) {
                break;
            }
            self.advance_one();
        }
        let Some(text_bytes) = self.src.get(start..self.pos) else {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                span_from(start, self.pos)?,
                "internal: identifier range invalid",
            ));
        };
        let text = match core::str::from_utf8(text_bytes) {
            Ok(s) => s,
            Err(_e) => {
                return Err(LqParseError::new(
                    LqParseErrorCode::TokenInvalid,
                    span_from(start, self.pos)?,
                    "invalid UTF-8 in identifier",
                ));
            }
        };
        if text.is_empty() {
            return Err(LqParseError::new(
                LqParseErrorCode::TokenInvalid,
                span_from(start, self.pos)?,
                "empty identifier",
            ));
        }
        let kind = match text {
            "AND" => LqTokenKind::And,
            "OR" => LqTokenKind::Or,
            "NOT" => LqTokenKind::Not,
            _ => {
                if text.starts_with('@') {
                    return Err(LqParseError::new(
                        LqParseErrorCode::ForbiddenSyntax,
                        span_from(start, self.pos)?,
                        "generic @ shorthand forbidden",
                    ));
                }
                if text.starts_with('~') {
                    return Err(LqParseError::new(
                        LqParseErrorCode::ForbiddenSyntax,
                        span_from(start, self.pos)?,
                        "fuzzy operator forbidden",
                    ));
                }
                LqTokenKind::KeywordOrFilterName(text.to_owned())
            }
        };
        Ok(LqToken {
            kind,
            span: span_from(start, self.pos)?,
        })
    }

    fn read_filter_value(&mut self, start: usize) -> Result<LqToken, LqParseError> {
        while let Some(&b) = self.src.get(self.pos) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'(' | b')') {
                break;
            }
            self.advance_one();
        }
        let Some(text_bytes) = self.src.get(start..self.pos) else {
            return Err(LqParseError::new(
                LqParseErrorCode::SyntaxError,
                span_from(start, self.pos)?,
                "internal: filter-value range invalid",
            ));
        };
        let text = match core::str::from_utf8(text_bytes) {
            Ok(s) => s,
            Err(_e) => {
                return Err(LqParseError::new(
                    LqParseErrorCode::TokenInvalid,
                    span_from(start, self.pos)?,
                    "invalid UTF-8 in filter value",
                ));
            }
        };
        if text.is_empty() {
            return Err(LqParseError::new(
                LqParseErrorCode::InvalidFilterValue,
                span_from(start, self.pos)?,
                "empty filter value",
            ));
        }
        Ok(LqToken {
            kind: LqTokenKind::ColonValue(text.to_owned()),
            span: span_from(start, self.pos)?,
        })
    }
}

/// Decode one UTF-8 code point at `bytes[at..]`. Returns `None` if invalid.
fn decode_utf8(bytes: &[u8], at: usize) -> Option<char> {
    let rest = bytes.get(at..)?;
    core::str::from_utf8(rest).map_or(None, |s| s.chars().next())
}

#[cfg(test)]
mod tests {
    use super::{LqTokenKind, tokenize};
    use crate::errors::LqParseErrorCode;
    use crate::limits::MAX_INPUT_BYTES;

    fn ok_kinds(input: &str) -> Vec<LqTokenKind> {
        match tokenize(input) {
            Ok(v) => v.into_iter().map(|t| t.kind).collect(),
            Err(e) => {
                assert!(false, "tokenize({input:?}) failed: {e}");
                Vec::new()
            }
        }
    }

    fn err_code(input: &str) -> LqParseErrorCode {
        match tokenize(input) {
            Ok(_) => {
                assert!(false, "tokenize({input:?}) unexpectedly succeeded");
                LqParseErrorCode::SyntaxError
            }
            Err(e) => e.code,
        }
    }

    #[test]
    fn bare_keyword_lexes_to_keyword_or_filter_name() {
        assert_eq!(
            ok_kinds("fooBar"),
            vec![
                LqTokenKind::KeywordOrFilterName("fooBar".to_owned()),
                LqTokenKind::Eof,
            ]
        );
    }

    #[test]
    fn phrase_with_escape_decodes() {
        assert_eq!(
            ok_kinds(r#""async\tfn""#),
            vec![
                LqTokenKind::Phrase("async\tfn".to_owned()),
                LqTokenKind::Eof
            ]
        );
    }

    #[test]
    fn raw_string_preserves_backslash() {
        assert_eq!(
            ok_kinds(r"'C:\Users'"),
            vec![
                LqTokenKind::RawString(r"C:\Users".to_owned()),
                LqTokenKind::Eof,
            ]
        );
    }

    #[test]
    fn regex_lexes_with_internal_escape() {
        assert_eq!(
            ok_kinds(r"/fn\s+\w+/"),
            vec![LqTokenKind::Regex(r"fn\s+\w+".to_owned()), LqTokenKind::Eof,]
        );
    }

    #[test]
    fn filter_lexes_to_identifier_colon_value() {
        assert_eq!(
            ok_kinds("repo:foo"),
            vec![
                LqTokenKind::KeywordOrFilterName("repo".to_owned()),
                LqTokenKind::Colon,
                LqTokenKind::ColonValue("foo".to_owned()),
                LqTokenKind::Eof,
            ]
        );
    }

    #[test]
    fn boolean_keywords_lex_distinctly() {
        assert_eq!(
            ok_kinds("a AND b OR NOT c"),
            vec![
                LqTokenKind::KeywordOrFilterName("a".to_owned()),
                LqTokenKind::And,
                LqTokenKind::KeywordOrFilterName("b".to_owned()),
                LqTokenKind::Or,
                LqTokenKind::Not,
                LqTokenKind::KeywordOrFilterName("c".to_owned()),
                LqTokenKind::Eof,
            ]
        );
    }

    #[test]
    fn parens_and_dash_negation_at_atom_start() {
        assert_eq!(
            ok_kinds("(a -b)"),
            vec![
                LqTokenKind::LParen,
                LqTokenKind::KeywordOrFilterName("a".to_owned()),
                LqTokenKind::Dash,
                LqTokenKind::KeywordOrFilterName("b".to_owned()),
                LqTokenKind::RParen,
                LqTokenKind::Eof,
            ]
        );
    }

    #[test]
    fn intra_word_dash_stays_in_identifier() {
        assert_eq!(
            ok_kinds("kebab-case"),
            vec![
                LqTokenKind::KeywordOrFilterName("kebab-case".to_owned()),
                LqTokenKind::Eof,
            ]
        );
    }

    #[test]
    fn match_block_captures_body() {
        assert_eq!(
            ok_kinds("match { fn $X(...) { } }"),
            vec![
                LqTokenKind::StructuralBlock(" fn $X(...) { } ".to_owned()),
                LqTokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unclosed_phrase_returns_unclosed_quote() {
        assert_eq!(
            err_code(r#""unterminated"#),
            LqParseErrorCode::UnclosedQuote
        );
    }

    #[test]
    fn unknown_phrase_escape_returns_token_invalid() {
        assert_eq!(err_code(r#""bad\x""#), LqParseErrorCode::TokenInvalid);
    }

    #[test]
    fn oversized_input_returns_limit_exceeded_bytes() {
        let blob = "a".repeat(MAX_INPUT_BYTES.saturating_add(1));
        assert_eq!(err_code(&blob), LqParseErrorCode::LimitExceededBytes);
    }

    #[test]
    fn at_shorthand_is_forbidden() {
        assert_eq!(err_code("@lang=rust"), LqParseErrorCode::ForbiddenSyntax);
    }

    #[test]
    fn tilde_fuzzy_is_forbidden() {
        assert_eq!(err_code("~similar"), LqParseErrorCode::ForbiddenSyntax);
    }
}
