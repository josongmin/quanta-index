//! AST-level dialect filter — precise rejection of the three forbidden
//! constructs whose detection requires the `regex_syntax::ast` layer:
//!
//! - **Possessive group `(?>…)`** — `regex_syntax` does not parse this
//!   syntax at all; the AST parser surfaces it as `FlagUnrecognized` with
//!   the offending span pointing at `>`. The previous slice-based
//!   heuristic in [`crate::dialect::classify_construct_from_slice`] could
//!   not see `(?>` from the 1-byte error span, so possessives were
//!   silently mis-classified as `ParseFail`. This module inspects the
//!   bytes of the input pattern immediately preceding the error span to
//!   confirm `(?>` and surface the typed [`ForbiddenKind::Possessive`].
//! - **Named-capture-ref `\k<name>`** — same shape: `regex_syntax`
//!   surfaces this as `EscapeUnrecognized` with a 2-byte span (`\k`). The
//!   typed slice is reliable here, but we re-validate explicitly so the
//!   classification is anchored to actual pattern bytes, not the error
//!   message.
//! - **Mid-pattern inline-flag switch `(?i)`** — `regex_syntax` accepts
//!   this construct unconditionally and produces an `Ast::Flags(SetFlags)`
//!   AST node inside the surrounding `Concat`. Walking the HIR cannot
//!   recover the construct (flags have already been folded into class
//!   `case_insensitive` bits). The AST walk inspects the top-level
//!   `Concat` and rejects any `Ast::Flags` node that does not appear at
//!   index 0 of that concatenation. Leading position is **accepted** as a
//!   canonicalization opportunity (PRE-NORM strips it before tokenizer
//!   handoff).
//!
//! Detection order at compile time (per [`crate::executor`]):
//!
//! 1. **AST walk** — this module. Fires FIRST for `Possessive`,
//!    `NamedCaptureRef`, `InlineFlagMidPattern` so the [`ForbiddenKind`]
//!    is precise.
//! 2. **HIR dialect filter** — [`crate::dialect::dialect_filter`]. Hook
//!    for unicode-class policy and other HIR-level rejections.
//! 3. NFA estimator → `regex::Regex::new`.
//!
//! AST-level vs HIR-level rationale: lookahead / lookbehind / backref are
//! rejected by `regex_syntax::ast` parse itself with typed
//! `UnsupportedLookAround` / `UnsupportedBackreference` error kinds, so
//! the existing [`crate::dialect::classify_ast_error`] path captures them
//! at the parse-error layer (not this walk). Forbidden constructs that
//! either (a) cannot be parsed at all by `regex_syntax` but use a generic
//! error kind (possessive, named-capture-ref) or (b) parse successfully
//! into an AST node `regex_syntax` allows but our dialect forbids
//! (mid-pattern inline flag) require this walk.

use regex_syntax::ast::Ast;
use regex_syntax::ast::ErrorKind as AstErrorKind;
use regex_syntax::ast::parse::Parser as AstParser;

use crate::errors::{ForbiddenKind, RegexError, RegexErrorCode};

/// Run the AST-level dialect filter over `pattern`.
///
/// Returns `Ok(())` when the pattern contains none of the three
/// constructs detected at this layer. Returns a typed
/// [`RegexErrorCode::ForbiddenSyntax`] with a precise [`ForbiddenKind`]
/// when one is found. Returns [`RegexErrorCode::ParseFail`] only when the
/// AST parse fails for an unrelated syntax reason (e.g. unclosed group).
///
/// Forbidden constructs whose AST-stage error kind is itself typed
/// (`UnsupportedLookAround`, `UnsupportedBackreference`) are intentionally
/// **not** classified here — they remain the responsibility of the
/// parse-error path in [`crate::executor`], because surfacing them twice
/// would duplicate logic. This function only handles the three constructs
/// where AST-stage error kinds are insufficient.
///
/// # Errors
///
/// - `FORBIDDEN_SYNTAX(possessive)` when the pattern contains `(?>`;
/// - `FORBIDDEN_SYNTAX(named-capture-ref)` when the pattern contains
///   `\k<…>`;
/// - `FORBIDDEN_SYNTAX(inline-flag-midpattern)` when an inline-flag
///   `(?i)` / `(?m)` / `(?s)` / `(?x)` / `(?U)` / `(?u)` / `(?R)` /
///   `(?-i)` (i.e. any `(?flags)` set-flag) appears at any position
///   **other than** index 0 of the top-level concatenation.
pub fn ast_walk_filter(pattern: &str) -> Result<(), RegexError> {
    match AstParser::new().parse(pattern) {
        Ok(ast) => walk_for_inline_flag(&ast),
        Err(e) => classify_parse_failure(pattern, &e),
    }
}

/// On AST parse failure, surface a typed `Possessive` / `NamedCaptureRef`.
///
/// Every other parse failure (including `UnsupportedLookAround` and
/// `UnsupportedBackreference`) is left for the executor's `parse_hir`
/// path to surface, to avoid duplicating the lookaround/backref typed
/// classification across two layers.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "AstErrorKind is non_exhaustive; non-possessive / non-named-capture-ref parse failures are surfaced by the downstream `parse_hir` path, not duplicated here"
)]
fn classify_parse_failure(pattern: &str, err: &regex_syntax::ast::Error) -> Result<(), RegexError> {
    let span = err.span();
    let start = span.start.offset;
    let bytes = pattern.as_bytes();
    match *err.kind() {
        AstErrorKind::FlagUnrecognized => {
            // `(?>abc)` surfaces as `FlagUnrecognized` at the `>` byte.
            // Confirm the two preceding bytes are `(?` by reading the
            // pattern bytes directly (the error slice itself is only the
            // `>` byte, which is why the legacy heuristic missed this).
            if is_possessive_open(bytes, start) {
                return Err(RegexError::forbidden(
                    ForbiddenKind::Possessive,
                    format!(
                        "regex dialect rejection at byte {start}: possessive `(?>` is forbidden"
                    ),
                ));
            }
            Ok(())
        }
        AstErrorKind::EscapeUnrecognized => {
            // `\k<name>` surfaces as `EscapeUnrecognized` with a 2-byte
            // span covering `\k`. Confirm by re-reading the pattern
            // bytes.
            if is_named_capture_ref_open(bytes, start) {
                return Err(RegexError::forbidden(
                    ForbiddenKind::NamedCaptureRef,
                    format!(
                        "regex dialect rejection at byte {start}: `\\k<name>` named-capture reference is forbidden"
                    ),
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Confirm `bytes[start-2 .. start+1]` is the ASCII string `(?>`.
///
/// Uses bounded `get` reads (no indexing/slicing) to satisfy the
/// workspace `clippy::indexing_slicing` deny.
fn is_possessive_open(bytes: &[u8], start: usize) -> bool {
    if start < 2 {
        return false;
    }
    let a = start.checked_sub(2);
    let b = start.checked_sub(1);
    let (Some(a), Some(b)) = (a, b) else {
        return false;
    };
    bytes.get(a).copied() == Some(b'(')
        && bytes.get(b).copied() == Some(b'?')
        && bytes.get(start).copied() == Some(b'>')
}

/// Confirm `bytes[start .. start+2]` is the ASCII string `\k`.
fn is_named_capture_ref_open(bytes: &[u8], start: usize) -> bool {
    let next = start.checked_add(1);
    let Some(next) = next else { return false };
    bytes.get(start).copied() == Some(b'\\') && bytes.get(next).copied() == Some(b'k')
}

/// Walk the AST and reject any `Ast::Flags(_)` set-flag node that is
/// **not** at the leading position of the top-level concatenation.
///
/// Decisions:
///
/// - A top-level `Ast::Flags(_)` (pattern is exactly `(?i)`) is rejected:
///   such a pattern matches the empty string and has no semantic content
///   beyond the flag switch. We treat it as a degenerate mid-pattern
///   switch.
/// - A top-level `Ast::Concat` with `Ast::Flags(_)` at index 0 and the
///   rest of the concat free of further `Ast::Flags` is **accepted**.
///   This is the canonicalization-friendly shape PRE-NORM strips before
///   handoff.
/// - Any `Ast::Flags(_)` at index `i > 0` of the top-level concat is
///   rejected as [`ForbiddenKind::InlineFlagMidPattern`].
/// - Any `Ast::Flags(_)` nested inside an `Ast::Alternation`,
///   `Ast::Group`, `Ast::Repetition`, etc. is rejected as
///   `InlineFlagMidPattern`.
///   (Scoped flags `(?i:foo)` use `Ast::Group { kind: NonCapturing(Flags) }`,
///   a different AST shape entirely — those remain allowed.)
fn walk_for_inline_flag(ast: &Ast) -> Result<(), RegexError> {
    match *ast {
        Ast::Concat(ref c) => {
            // Inspect each child. Index 0 may be `Ast::Flags(_)`; any
            // other position with `Ast::Flags(_)` is a mid-pattern flag
            // switch. All children are also recursively walked to catch
            // nested inline-flag switches inside groups/repetitions/
            // alternations.
            for (idx, child) in c.asts.iter().enumerate() {
                match *child {
                    Ast::Flags(ref sf) => {
                        if idx != 0 {
                            return Err(inline_flag_error(sf.span.start.offset));
                        }
                        // Leading `(?i)` accepted — canonicalization
                        // hand-off to PRE-NORM.
                    }
                    Ast::Empty(_)
                    | Ast::Literal(_)
                    | Ast::Dot(_)
                    | Ast::Assertion(_)
                    | Ast::ClassUnicode(_)
                    | Ast::ClassPerl(_)
                    | Ast::ClassBracketed(_)
                    | Ast::Repetition(_)
                    | Ast::Group(_)
                    | Ast::Alternation(_)
                    | Ast::Concat(_) => walk_disallow_any_flags(child)?,
                }
            }
            Ok(())
        }
        // Top-level lone `Ast::Flags(_)` — degenerate `(?i)` pattern with
        // nothing else. Treat as mid-pattern: there is no body to apply
        // the flag to. (PRE-NORM would have stripped a leading `(?i)`
        // before this, leaving `Ast::Empty`, not `Ast::Flags`.)
        Ast::Flags(ref sf) => Err(inline_flag_error(sf.span.start.offset)),
        // Everything else: recurse normally; reject any flag node
        // encountered at any depth.
        Ast::Empty(_)
        | Ast::Literal(_)
        | Ast::Dot(_)
        | Ast::Assertion(_)
        | Ast::ClassUnicode(_)
        | Ast::ClassPerl(_)
        | Ast::ClassBracketed(_)
        | Ast::Repetition(_)
        | Ast::Group(_)
        | Ast::Alternation(_) => walk_disallow_any_flags(ast),
    }
}

/// Recursive walk that rejects every `Ast::Flags(_)` it finds (used for
/// nested positions where no inline-flag switch is ever allowed).
fn walk_disallow_any_flags(ast: &Ast) -> Result<(), RegexError> {
    match *ast {
        Ast::Empty(_)
        | Ast::Literal(_)
        | Ast::Dot(_)
        | Ast::Assertion(_)
        | Ast::ClassUnicode(_)
        | Ast::ClassPerl(_)
        | Ast::ClassBracketed(_) => Ok(()),
        Ast::Flags(ref sf) => Err(inline_flag_error(sf.span.start.offset)),
        Ast::Repetition(ref r) => walk_disallow_any_flags(&r.ast),
        Ast::Group(ref g) => walk_disallow_any_flags(&g.ast),
        Ast::Alternation(ref a) => {
            for child in &a.asts {
                walk_disallow_any_flags(child)?;
            }
            Ok(())
        }
        Ast::Concat(ref c) => {
            for child in &c.asts {
                walk_disallow_any_flags(child)?;
            }
            Ok(())
        }
    }
}

fn inline_flag_error(byte_offset: usize) -> RegexError {
    RegexError {
        code: RegexErrorCode::ForbiddenSyntax,
        dimension: None,
        forbidden: Some(ForbiddenKind::InlineFlagMidPattern),
        detail: format!(
            "regex dialect rejection at byte {byte_offset}: mid-pattern inline-flag switch is forbidden (leading `(?i)` is the only accepted form)"
        ).into_boxed_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::ast_walk_filter;
    use crate::errors::{ForbiddenKind, RegexErrorCode};

    #[test]
    fn accepts_plain_literal() {
        match ast_walk_filter("foo") {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn accepts_leading_inline_flag() {
        // Leading `(?i)` is canonicalization-friendly; PRE-NORM strips
        // it. The AST walk must accept it so the downstream pipeline
        // sees the same shape as a plain `foo` pattern.
        match ast_walk_filter("(?i)foo") {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn accepts_scoped_inline_flag_group() {
        // `(?i:foo)` is a scoped non-capturing group with flags — a
        // different AST shape than `(?i)foo`. This is permitted.
        match ast_walk_filter("(?i:foo)") {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn rejects_mid_pattern_inline_flag() {
        match ast_walk_filter("foo(?i)bar") {
            Ok(()) => assert!(false, "expected ForbiddenSyntax(InlineFlagMidPattern)"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::InlineFlagMidPattern));
            }
        }
    }

    #[test]
    fn rejects_second_inline_flag_after_leading() {
        // `(?i)foo(?m)bar` — leading flag accepted, second flag rejected.
        match ast_walk_filter("(?i)foo(?m)bar") {
            Ok(()) => assert!(false, "expected ForbiddenSyntax(InlineFlagMidPattern)"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::InlineFlagMidPattern));
            }
        }
    }

    #[test]
    fn rejects_inline_flag_inside_group() {
        // `(foo(?i)bar)` — inline flag inside capturing group. Reject.
        match ast_walk_filter("(foo(?i)bar)") {
            Ok(()) => assert!(false, "expected ForbiddenSyntax(InlineFlagMidPattern)"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::InlineFlagMidPattern));
            }
        }
    }

    #[test]
    fn rejects_inline_flag_inside_alternation() {
        match ast_walk_filter("a|(?i)b") {
            Ok(()) => assert!(false, "expected ForbiddenSyntax(InlineFlagMidPattern)"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::InlineFlagMidPattern));
            }
        }
    }

    #[test]
    fn rejects_possessive_group() {
        match ast_walk_filter("(?>abc)") {
            Ok(()) => assert!(false, "expected ForbiddenSyntax(Possessive)"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::Possessive));
            }
        }
    }

    #[test]
    fn rejects_possessive_group_not_at_start() {
        // `(foo)(?>bar)` — possessive after a normal group. Detection
        // must use absolute pattern offsets, not relative to start.
        match ast_walk_filter("(foo)(?>bar)") {
            Ok(()) => assert!(false, "expected ForbiddenSyntax(Possessive)"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::Possessive));
            }
        }
    }

    #[test]
    fn rejects_named_capture_ref() {
        match ast_walk_filter(r"\b\k<x>\b") {
            Ok(()) => assert!(false, "expected ForbiddenSyntax(NamedCaptureRef)"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::NamedCaptureRef));
            }
        }
    }

    #[test]
    fn rejects_named_capture_ref_alone() {
        match ast_walk_filter(r"\k<x>") {
            Ok(()) => assert!(false, "expected ForbiddenSyntax(NamedCaptureRef)"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::NamedCaptureRef));
            }
        }
    }

    #[test]
    fn unrelated_parse_failure_returns_ok() {
        // `foo(` is an unclosed group. The AST walk leaves this for the
        // downstream `parse_hir` path to surface as `ParseFail`. We
        // verify we return `Ok(())` (no forbidden classification).
        match ast_walk_filter("foo(") {
            Ok(()) => {}
            Err(e) => {
                assert!(
                    false,
                    "expected Ok(()) (downstream parse_hir surfaces ParseFail), got {e}"
                );
            }
        }
    }

    #[test]
    fn lookahead_left_to_downstream() {
        // Lookahead is `UnsupportedLookAround` at AST stage; we leave
        // its typed classification to `classify_ast_error`.
        match ast_walk_filter("foo(?=bar)") {
            Ok(()) => {}
            Err(e) => {
                assert!(
                    false,
                    "AST walk should not classify lookaround; expected Ok(()), got {e}"
                );
            }
        }
    }

    #[test]
    fn backref_left_to_downstream() {
        match ast_walk_filter(r"(foo)\1") {
            Ok(()) => {}
            Err(e) => {
                assert!(
                    false,
                    "AST walk should not classify backref; expected Ok(()), got {e}"
                );
            }
        }
    }

    #[test]
    fn deeply_nested_inline_flag_rejected() {
        match ast_walk_filter(r"((a|b)*(?i)c)") {
            Ok(()) => assert!(false, "expected InlineFlagMidPattern"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::InlineFlagMidPattern));
            }
        }
    }

    #[test]
    fn accepts_anchors_and_classes() {
        for p in &[
            r"^fn foo",
            r"\bfoo\b",
            r"[a-z]+",
            r"foo|bar|baz",
            r"fn\s+\w+",
        ] {
            match ast_walk_filter(p) {
                Ok(()) => {}
                Err(e) => assert!(false, "{p:?}: {e}"),
            }
        }
    }
}
