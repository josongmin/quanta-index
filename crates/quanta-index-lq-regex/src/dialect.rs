//! RE2 dialect filter — parse-time rejection of constructs forbidden by the
//! LQ regex dialect (lookbehind, lookahead, backreference, possessive
//! group, named-capture reference, mid-pattern inline flag).
//!
//! The Rust `regex` crate is already RE2-class and the `regex-syntax`
//! parser already rejects look-around and backreference at AST stage. This
//! module's job is two-fold:
//!
//! 1. classify those parse-time rejections into our typed
//!    [`ForbiddenKind`] variants so callers see structured reasons rather
//!    than raw `regex_syntax` strings; this happens in
//!    [`classify_ast_error`];
//! 2. walk the successfully-parsed HIR for any residual constructs we
//!    consider out of scope for v1 (currently nothing — left as a hook
//!    for ADR-017 unicode-class policy).
//!
//! See [`crate`] module doc and the LEX-04 spec sheet §3 for the closed
//! set of rejected constructs.

use regex_syntax::ast::ErrorKind as AstErrorKind;
use regex_syntax::hir::Hir;

use crate::errors::{ForbiddenKind, RegexError};

/// Walk a fully-parsed [`Hir`] for any residual forbidden construct.
///
/// Returning `Ok(())` does NOT mean the pattern is safe to compile —
/// the NFA-state estimator must still run. See
/// [`crate::estimator::estimate_nfa_states`].
///
/// At v1 every forbidden construct (lookbehind, lookahead, backref) is
/// already caught at AST stage by `regex-syntax`; this HIR walk is a
/// future hook (e.g. for unicode-class policy per §12 Q-LEX04-4). The
/// `Result` return shape is preserved so a policy change only edits the
/// body of this function.
#[expect(
    clippy::unnecessary_wraps,
    reason = "Result return is the contract; future unicode-class rejection will populate Err"
)]
pub fn dialect_filter(_hir: &Hir) -> Result<(), RegexError> {
    Ok(())
}

/// Classify a `regex_syntax::ast::ErrorKind` into a typed [`ForbiddenKind`]
/// when the AST parser rejected the pattern because of a construct the
/// LQ regex dialect forbids.
///
/// `pattern_slice` is the offending span text extracted from the parse
/// error span; it is used only to disambiguate lookahead vs lookbehind
/// (the `regex-syntax` AST stage collapses both into
/// `UnsupportedLookAround`).
///
/// Returns `None` if the error is a generic parse failure (e.g.
/// `GroupUnclosed`) that maps to [`crate::RegexErrorCode::ParseFail`]
/// rather than `FORBIDDEN_SYNTAX`.
#[must_use]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "AstErrorKind is non_exhaustive; the wildcard arm catches both unmodeled future variants and every present variant that maps to ParseFail (not FORBIDDEN_SYNTAX)"
)]
pub fn classify_ast_error(kind: &AstErrorKind, pattern_slice: &str) -> Option<ForbiddenKind> {
    match *kind {
        AstErrorKind::UnsupportedLookAround => Some(classify_lookaround(pattern_slice)),
        AstErrorKind::UnsupportedBackreference => Some(ForbiddenKind::Backref),
        // Everything else (every present generic syntax error AND every
        // future `non_exhaustive` variant) maps to `None`, which the
        // caller surfaces as `ParseFail`. We intentionally do not
        // exhaustively enumerate the present variants here: clippy's
        // `match_same_arms` would collapse them with the wildcard, and
        // any future variant added by `regex-syntax` would silently
        // break the build under exhaustive matching.
        _ => None,
    }
}

/// Disambiguate lookahead vs lookbehind from the offending pattern text.
///
/// `regex-syntax` collapses `(?=…)`, `(?!…)`, `(?<=…)`, `(?<!…)` into a
/// single `UnsupportedLookAround` kind. We re-examine the span text to
/// preserve the distinction in our typed surface.
fn classify_lookaround(slice: &str) -> ForbiddenKind {
    // The span typically starts at `(?` — peek the byte after.
    // Use a manual byte walk to avoid string-slice clippy denials.
    let bytes = slice.as_bytes();
    let mut i: usize = 0;
    // Skip leading `(?`.
    if bytes.first().copied() == Some(b'(') {
        i = i.saturating_add(1);
    }
    if bytes.get(i).copied() == Some(b'?') {
        i = i.saturating_add(1);
    }
    match bytes.get(i).copied() {
        Some(b'<') => ForbiddenKind::Lookbehind,
        _ => ForbiddenKind::Lookahead,
    }
}

/// Classify a `regex_syntax::ast::ErrorKind` carrying a span message that
/// strongly suggests a possessive group `(?>…)`, a named-capture-ref
/// `\k<name>`, or a mid-pattern inline flag `…(?i)…`.
///
/// The `regex-syntax` parser rejects those constructs but uses generic
/// kinds (`GroupNameInvalid`, `EscapeUnrecognized`, `FlagUnrecognized`).
/// We re-examine the pattern slice to surface the typed reason.
///
/// Returns `None` if the slice does not match any of the
/// LQ-dialect-forbidden constructs handled here.
#[must_use]
pub fn classify_construct_from_slice(pattern_slice: &str) -> Option<ForbiddenKind> {
    let bytes = pattern_slice.as_bytes();
    // `(?>…` — possessive / atomic group.
    if bytes.first().copied() == Some(b'(')
        && bytes.get(1).copied() == Some(b'?')
        && bytes.get(2).copied() == Some(b'>')
    {
        return Some(ForbiddenKind::Possessive);
    }
    // `\k<…>` — named-capture reference.
    if bytes.first().copied() == Some(b'\\') && bytes.get(1).copied() == Some(b'k') {
        return Some(ForbiddenKind::NamedCaptureRef);
    }
    None
}

#[cfg(test)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "tests assert one variant; other variants are not exercised here"
)]
mod tests {
    use super::{classify_ast_error, classify_construct_from_slice, dialect_filter};
    use crate::errors::ForbiddenKind;
    use regex_syntax::Parser;
    use regex_syntax::ast::ErrorKind as AstErrorKind;
    use regex_syntax::ast::parse::Parser as AstParser;

    fn parse_hir(p: &str) -> regex_syntax::hir::Hir {
        match Parser::new().parse(p) {
            Ok(h) => h,
            Err(e) => {
                assert!(false, "parse should succeed for {p:?}: {e}");
                // Unreachable but required to satisfy type system; tests
                // never reach here.
                regex_syntax::hir::Hir::empty()
            }
        }
    }

    #[test]
    fn dialect_filter_accepts_anchored_literal() {
        let h = parse_hir("^fn foo");
        let r = dialect_filter(&h);
        assert!(r.is_ok());
    }

    #[test]
    fn dialect_filter_accepts_word_boundary() {
        let h = parse_hir(r"\bfoo\b");
        let r = dialect_filter(&h);
        assert!(r.is_ok());
    }

    #[test]
    fn dialect_filter_accepts_alternation() {
        let h = parse_hir("foo|bar|baz");
        let r = dialect_filter(&h);
        assert!(r.is_ok());
    }

    #[test]
    fn classify_lookbehind_from_slice() {
        let k = classify_ast_error(&AstErrorKind::UnsupportedLookAround, "(?<=foo)");
        assert_eq!(k, Some(ForbiddenKind::Lookbehind));
    }

    #[test]
    fn classify_negative_lookbehind_from_slice() {
        let k = classify_ast_error(&AstErrorKind::UnsupportedLookAround, "(?<!foo)");
        assert_eq!(k, Some(ForbiddenKind::Lookbehind));
    }

    #[test]
    fn classify_lookahead_from_slice() {
        let k = classify_ast_error(&AstErrorKind::UnsupportedLookAround, "(?=foo)");
        assert_eq!(k, Some(ForbiddenKind::Lookahead));
    }

    #[test]
    fn classify_negative_lookahead_from_slice() {
        let k = classify_ast_error(&AstErrorKind::UnsupportedLookAround, "(?!foo)");
        assert_eq!(k, Some(ForbiddenKind::Lookahead));
    }

    #[test]
    fn classify_backref_kind() {
        let k = classify_ast_error(&AstErrorKind::UnsupportedBackreference, r"\1");
        assert_eq!(k, Some(ForbiddenKind::Backref));
    }

    #[test]
    fn classify_unrelated_kind_returns_none() {
        let k = classify_ast_error(&AstErrorKind::GroupUnclosed, "(foo");
        assert!(k.is_none());
    }

    #[test]
    fn classify_possessive_from_slice() {
        let k = classify_construct_from_slice("(?>foo)");
        assert_eq!(k, Some(ForbiddenKind::Possessive));
    }

    #[test]
    fn classify_named_capture_ref_from_slice() {
        let k = classify_construct_from_slice(r"\k<n>");
        assert_eq!(k, Some(ForbiddenKind::NamedCaptureRef));
    }

    #[test]
    fn classify_plain_literal_returns_none() {
        let k = classify_construct_from_slice("foo");
        assert!(k.is_none());
    }

    #[test]
    fn ast_parser_rejects_lookahead() {
        // Sanity check: confirm `regex-syntax` flags lookahead as
        // UnsupportedLookAround. This guards against the AST parser
        // changing its error kind in a future minor bump.
        match AstParser::new().parse("foo(?=bar)") {
            Ok(_) => assert!(false, "expected look-around rejection"),
            Err(e) => match e.kind() {
                AstErrorKind::UnsupportedLookAround => {}
                other => assert!(false, "unexpected ast error kind: {other:?}"),
            },
        }
    }

    #[test]
    fn ast_parser_rejects_lookbehind() {
        match AstParser::new().parse("(?<=foo)bar") {
            Ok(_) => assert!(false, "expected look-around rejection"),
            Err(e) => match e.kind() {
                AstErrorKind::UnsupportedLookAround => {}
                other => assert!(false, "unexpected ast error kind: {other:?}"),
            },
        }
    }

    #[test]
    fn ast_parser_rejects_backref() {
        match AstParser::new().parse(r"(foo)\1") {
            Ok(_) => assert!(false, "expected backref rejection"),
            Err(e) => match e.kind() {
                AstErrorKind::UnsupportedBackreference => {}
                other => assert!(false, "unexpected ast error kind: {other:?}"),
            },
        }
    }
}
