//! Required-literal extractor for the trigram prefilter lane.
//!
//! [`extract_required_literals`] runs `regex_syntax::hir::literal::Extractor`
//! over an HIR and returns the resulting byte literals so the trigram
//! prefilter can intersect their per-trigram posting lists.
//!
//! Pure-wildcard patterns (`.*`, `\w+`) yield an infinite literal set —
//! the function surfaces typed [`crate::RegexErrorCode::RegexPrefilterUnusable`]
//! so the planner can drop to a verify-only path explicitly. There is
//! no silent fallback.

use regex_syntax::hir::Hir;
use regex_syntax::hir::literal::Extractor;

use crate::errors::{LimitDimension, RegexError, RegexErrorCode};

/// Cap on the total bytes summed across all extracted literals. 4 KiB
/// keeps the prefilter trigram set well below the trigram-side cap and
/// bounds the worst-case alternation explosion.
pub const MAX_LITERAL_TOTAL_BYTES: usize = 4_096;

/// Cap on the byte length of a single extracted literal.
///
/// Long literals past this are truncated by the extractor; this guard
/// surfaces a typed [`RegexErrorCode::PlanLimitExceeded`] when a single
/// literal is implausibly long.
pub const MAX_SINGLE_LITERAL_BYTES: usize = 1_024;

/// Run the `regex_syntax` literal extractor and return the mandatory
/// byte literals the trigram prefilter should AND against.
///
/// Behaviour:
///
/// - infinite seq (extractor returns `None`) →
///   [`RegexErrorCode::RegexPrefilterUnusable`]
/// - empty seq (matches nothing) →
///   [`RegexErrorCode::RegexPrefilterUnusable`]
/// - any single literal exceeds [`MAX_SINGLE_LITERAL_BYTES`] →
///   [`RegexErrorCode::PlanLimitExceeded`] with
///   [`LimitDimension::LiteralLen`]
/// - aggregate byte budget exceeds [`MAX_LITERAL_TOTAL_BYTES`] →
///   [`RegexErrorCode::PlanLimitExceeded`] with
///   [`LimitDimension::LiteralLen`]
pub fn extract_required_literals(hir: &Hir) -> Result<Vec<Vec<u8>>, RegexError> {
    let seq = Extractor::new().extract(hir);
    let Some(lits) = seq.literals() else {
        return Err(RegexError::new(
            RegexErrorCode::RegexPrefilterUnusable,
            "regex literal extractor returned infinite seq; verify-only path required",
        ));
    };
    if lits.is_empty() {
        return Err(RegexError::new(
            RegexErrorCode::RegexPrefilterUnusable,
            "regex literal extractor returned empty seq; pattern matches nothing or has no mandatory literal",
        ));
    }
    let mut out: Vec<Vec<u8>> = Vec::with_capacity(lits.len());
    let mut total: usize = 0;
    for lit in lits {
        let bytes = lit.as_bytes();
        if bytes.len() > MAX_SINGLE_LITERAL_BYTES {
            return Err(RegexError::plan_limit(
                LimitDimension::LiteralLen,
                format!(
                    "extracted literal length {} exceeds per-literal cap {}",
                    bytes.len(),
                    MAX_SINGLE_LITERAL_BYTES
                ),
            ));
        }
        total = total.checked_add(bytes.len()).ok_or_else(|| {
            RegexError::plan_limit(
                LimitDimension::LiteralLen,
                "aggregate literal byte total overflowed usize",
            )
        })?;
        if total > MAX_LITERAL_TOTAL_BYTES {
            return Err(RegexError::plan_limit(
                LimitDimension::LiteralLen,
                format!("aggregate literal bytes {total} exceeds cap {MAX_LITERAL_TOTAL_BYTES}"),
            ));
        }
        out.push(bytes.to_vec());
    }
    Ok(out)
}

#[cfg(test)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "tests assert one variant; other variants are not exercised here"
)]
mod tests {
    use super::extract_required_literals;
    use crate::errors::RegexErrorCode;
    use regex_syntax::Parser;

    fn parse(p: &str) -> regex_syntax::hir::Hir {
        match Parser::new().parse(p) {
            Ok(h) => h,
            Err(e) => {
                assert!(false, "parse failed for {p:?}: {e}");
                regex_syntax::hir::Hir::empty()
            }
        }
    }

    #[test]
    fn plain_literal_extracts_self() {
        let h = parse("foo");
        match extract_required_literals(&h) {
            Ok(v) => {
                assert!(!v.is_empty());
                // The extractor returns one or more literal candidates,
                // each of which must contain `foo`'s bytes somewhere.
                let any = v.iter().any(|l| l.as_slice() == b"foo");
                assert!(any, "expected `foo` among literals: {v:?}");
            }
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn anchored_word_boundary_extracts_literal() {
        let h = parse(r"\bfoo\b");
        match extract_required_literals(&h) {
            Ok(v) => {
                let any = v.iter().any(|l| l.as_slice() == b"foo");
                assert!(any, "expected `foo` among literals: {v:?}");
            }
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn alternation_yields_multiple_literals() {
        let h = parse("foo|bar|baz");
        match extract_required_literals(&h) {
            Ok(v) => {
                let foo = v.iter().any(|l| l.as_slice() == b"foo");
                let bar = v.iter().any(|l| l.as_slice() == b"bar");
                let baz = v.iter().any(|l| l.as_slice() == b"baz");
                assert!(foo && bar && baz, "expected all three: {v:?}");
            }
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn pure_wildcard_unusable() {
        let h = parse(".*");
        match extract_required_literals(&h) {
            Ok(v) => assert!(false, "expected REGEX_PREFILTER_UNUSABLE, got {v:?}"),
            Err(e) => assert_eq!(e.code, RegexErrorCode::RegexPrefilterUnusable),
        }
    }

    #[test]
    fn word_class_repeat_unusable() {
        let h = parse(r"\w+");
        match extract_required_literals(&h) {
            Ok(_) => assert!(false, "expected REGEX_PREFILTER_UNUSABLE"),
            Err(e) => assert_eq!(e.code, RegexErrorCode::RegexPrefilterUnusable),
        }
    }

    #[test]
    fn fn_handle_word_extracts_fn_prefix() {
        let h = parse(r"fn\s+handle_\w+");
        match extract_required_literals(&h) {
            Ok(v) => {
                // The extractor may return the prefix variants `fn ` or
                // longer literals; we just assert the set is non-empty
                // and contains some literal starting with `fn`.
                let any = v.iter().any(|l| l.starts_with(b"fn"));
                assert!(any, "expected literal starting with `fn`: {v:?}");
            }
            Err(e) => match e.code {
                RegexErrorCode::RegexPrefilterUnusable => {
                    // Acceptable: the extractor may decide the pattern's
                    // literal set is too broad. The verify-only path is
                    // then engaged by the caller.
                }
                _ => assert!(false, "unexpected error: {e}"),
            },
        }
    }
}
