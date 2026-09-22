//! Regex pre-compile guard.
//!
//! Parses a `/.../` regex source through `regex_syntax` and walks the
//! resulting HIR to estimate an upper bound on the NFA state count the
//! `regex` crate would emit. Inputs that exceed
//! [`crate::limits::MAX_NFA_STATES`] are rejected with
//! [`LqParseErrorCode::LimitExceededNfa`]; inputs containing constructs
//! forbidden by the RFC (lookbehind, backreference, possessive) surface
//! [`LqParseErrorCode::ForbiddenSyntax`].
//!
//! The estimator is intentionally monotone — every recursive subtree adds
//! at least one state, and bounded repetitions multiply through. The shape
//! mirrors `quanta-index-lq-regex::estimator::estimate_nfa_states` but is
//! duplicated here on purpose: the contract-layer integration ticket will
//! align the two crates. Importing it now would couple PRE-NORM to the
//! regex plane.

use regex_syntax::Parser;
use regex_syntax::ast::ErrorKind as AstErrorKind;
use regex_syntax::hir::{Hir, HirKind, Repetition};

use crate::errors::{LqParseError, LqParseErrorCode, LqSpan};
use crate::limits::MAX_NFA_STATES;

/// Pre-check a regex source string before it ever hits the `regex` crate.
///
/// Returns `Ok(())` if the pattern is grammar-legal under the LQ subset
/// and the NFA-state upper bound is within
/// [`crate::limits::MAX_NFA_STATES`].
///
/// The `span` argument is the source-anchor span of the regex leaf so
/// callers can attribute the failure back to its position in the raw
/// input.
pub fn precheck_regex(raw: &str, span: LqSpan) -> Result<(), LqParseError> {
    let hir = match Parser::new().parse(raw) {
        Ok(h) => h,
        Err(e) => return Err(map_parse_error(&e, span)),
    };
    let _states = estimate_nfa_states(&hir, span)?;
    Ok(())
}

/// Map a `regex_syntax::Error` into the closed LQ taxonomy.
///
/// Lookaround and backreference constructs are surfaced as
/// [`LqParseErrorCode::ForbiddenSyntax`] per the RFC; everything else
/// the `regex_syntax` parser surfaces collapses to
/// [`LqParseErrorCode::RegexParse`]. Both `regex_syntax::Error` and
/// `regex_syntax::ast::ErrorKind` are `#[non_exhaustive]`, so the
/// fallback wildcard is required for forward compatibility.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "regex_syntax::Error and ast::ErrorKind are #[non_exhaustive]; the wildcard preserves the typed RegexParse surface across future variants"
)]
fn map_parse_error(e: &regex_syntax::Error, span: LqSpan) -> LqParseError {
    if let regex_syntax::Error::Parse(p) = e {
        match p.kind() {
            AstErrorKind::UnsupportedLookAround | AstErrorKind::UnsupportedBackreference => {
                return LqParseError::new(
                    LqParseErrorCode::ForbiddenSyntax,
                    span,
                    format!("regex construct forbidden: {e}"),
                );
            }
            _ => {}
        }
    }
    LqParseError::new(
        LqParseErrorCode::RegexParse,
        span,
        format!("regex parse error: {e}"),
    )
}

/// Walk the HIR and return a monotone upper bound on the NFA state count.
///
/// Cap-exceed produces [`LqParseErrorCode::LimitExceededNfa`].
fn estimate_nfa_states(hir: &Hir, span: LqSpan) -> Result<u64, LqParseError> {
    let n = walk(hir, span)?;
    if n > MAX_NFA_STATES_U64 {
        return Err(LqParseError::new(
            LqParseErrorCode::LimitExceededNfa,
            span,
            format!("estimated NFA states {n} exceeds cap {MAX_NFA_STATES}"),
        ));
    }
    Ok(n)
}

/// The `MAX_NFA_STATES` cap widened to `u64` for arithmetic against
/// running sums in the HIR walk. Widening from `u32` is always exact.
#[expect(
    clippy::as_conversions,
    reason = "const-context u32 -> u64 widening; `u64::from(u32)` is not yet const-stable on the MSRV"
)]
const MAX_NFA_STATES_U64: u64 = MAX_NFA_STATES as u64;

fn walk(hir: &Hir, span: LqSpan) -> Result<u64, LqParseError> {
    match hir.kind() {
        HirKind::Empty | HirKind::Look(_) => Ok(1),
        HirKind::Literal(lit) => {
            let len = u64::try_from(lit.0.len()).map_err(|e| {
                LqParseError::new(
                    LqParseErrorCode::LimitExceededNfa,
                    span,
                    format!("literal length overflow u64: {e}"),
                )
            })?;
            len.checked_add(1).ok_or_else(|| {
                LqParseError::new(
                    LqParseErrorCode::LimitExceededNfa,
                    span,
                    "literal length + 1 overflowed u64",
                )
            })
        }
        HirKind::Class(cls) => {
            let ranges = class_range_count(cls, span)?;
            ranges.checked_add(1).ok_or_else(|| {
                LqParseError::new(
                    LqParseErrorCode::LimitExceededNfa,
                    span,
                    "class range count + 1 overflowed u64",
                )
            })
        }
        HirKind::Repetition(rep) => repetition_bound(rep, span),
        HirKind::Capture(cap) => {
            let sub = walk(&cap.sub, span)?;
            sub.checked_add(2).ok_or_else(|| {
                LqParseError::new(
                    LqParseErrorCode::LimitExceededNfa,
                    span,
                    "capture body + 2 overflowed u64",
                )
            })
        }
        HirKind::Concat(subs) => {
            let mut total: u64 = 1;
            for s in subs {
                let v = walk(s, span)?;
                total = total.checked_add(v).ok_or_else(|| {
                    LqParseError::new(
                        LqParseErrorCode::LimitExceededNfa,
                        span,
                        "concat sum overflowed u64",
                    )
                })?;
                if total > MAX_NFA_STATES_U64 {
                    return Err(LqParseError::new(
                        LqParseErrorCode::LimitExceededNfa,
                        span,
                        format!("concat running sum {total} exceeds cap {MAX_NFA_STATES}"),
                    ));
                }
            }
            Ok(total)
        }
        HirKind::Alternation(subs) => {
            let mut total: u64 = 1;
            for s in subs {
                let v = walk(s, span)?;
                total = total.checked_add(v).ok_or_else(|| {
                    LqParseError::new(
                        LqParseErrorCode::LimitExceededNfa,
                        span,
                        "alternation sum overflowed u64",
                    )
                })?;
                if total > MAX_NFA_STATES_U64 {
                    return Err(LqParseError::new(
                        LqParseErrorCode::LimitExceededNfa,
                        span,
                        format!("alternation running sum {total} exceeds cap {MAX_NFA_STATES}"),
                    ));
                }
            }
            Ok(total)
        }
    }
}

fn class_range_count(cls: &regex_syntax::hir::Class, span: LqSpan) -> Result<u64, LqParseError> {
    let n = match *cls {
        regex_syntax::hir::Class::Unicode(ref u) => u.ranges().len(),
        regex_syntax::hir::Class::Bytes(ref b) => b.ranges().len(),
    };
    u64::try_from(n).map_err(|e| {
        LqParseError::new(
            LqParseErrorCode::LimitExceededNfa,
            span,
            format!("class range count overflow u64: {e}"),
        )
    })
}

fn repetition_bound(rep: &Repetition, span: LqSpan) -> Result<u64, LqParseError> {
    let body = walk(&rep.sub, span)?;
    let max_factor: u64 = rep.max.map_or_else(
        || u64::from(rep.min).saturating_add(1),
        |m| {
            let cap = MAX_NFA_STATES_U64
                .checked_add(1)
                .unwrap_or(MAX_NFA_STATES_U64);
            let m_u64 = u64::from(m);
            if m_u64 > cap { cap } else { m_u64 }
        },
    );
    let body_states = body.checked_add(1).ok_or_else(|| {
        LqParseError::new(
            LqParseErrorCode::LimitExceededNfa,
            span,
            "repetition body + 1 overflowed u64",
        )
    })?;
    let total = body_states.checked_mul(max_factor).ok_or_else(|| {
        LqParseError::new(
            LqParseErrorCode::LimitExceededNfa,
            span,
            "repetition body * max overflowed u64",
        )
    })?;
    if total > MAX_NFA_STATES_U64 {
        return Err(LqParseError::new(
            LqParseErrorCode::LimitExceededNfa,
            span,
            format!(
                "repetition body {body_states} * max {max_factor} = {total} exceeds cap {MAX_NFA_STATES}"
            ),
        ));
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::precheck_regex;
    use crate::errors::{LqParseErrorCode, LqSpan};

    fn err_code(raw: &str) -> LqParseErrorCode {
        match precheck_regex(raw, LqSpan::new(0, 0)) {
            Ok(()) => {
                assert!(false, "precheck_regex({raw:?}) unexpectedly succeeded");
                LqParseErrorCode::RegexParse
            }
            Err(e) => e.code,
        }
    }

    #[test]
    fn small_literal_accepts() {
        match precheck_regex("foo", LqSpan::new(0, 0)) {
            Ok(()) => {}
            Err(e) => {
                assert!(false, "expected ok, got {e}");
            }
        }
    }

    #[test]
    fn word_boundary_anchor_accepts() {
        match precheck_regex(r"\bfoo\b", LqSpan::new(0, 0)) {
            Ok(()) => {}
            Err(e) => {
                assert!(false, "expected ok, got {e}");
            }
        }
    }

    #[test]
    fn stacked_bounded_repetition_overflows_nfa_cap() {
        // From the regex estimator's golden case; the conservative walk
        // sums hundreds of thousands of states for this construct.
        let raw = "a{0,1000}b{0,1000}c{0,1000}d{0,1000}e{0,1000}f{0,1000}g{0,1000}h{0,1000}i{0,1000}j{0,1000}k{0,1000}l{0,1000}m{0,1000}n{0,1000}o{0,1000}p{0,1000}q{0,1000}r{0,1000}s{0,1000}t{0,1000}u{0,1000}v{0,1000}w{0,1000}x{0,1000}y{0,1000}z{0,1000}A{0,1000}B{0,1000}C{0,1000}D{0,1000}E{0,1000}F{0,1000}G{0,1000}H{0,1000}I{0,1000}J{0,1000}K{0,1000}L{0,1000}M{0,1000}N{0,1000}O{0,1000}P{0,1000}Q{0,1000}R{0,1000}S{0,1000}T{0,1000}U{0,1000}V{0,1000}W{0,1000}X{0,1000}Y{0,1000}Z{0,1000}0{0,1000}1{0,1000}2{0,1000}3{0,1000}3{0,1000}5{0,1000}6{0,1000}7{0,1000}8{0,1000}9{0,1000}";
        assert_eq!(err_code(raw), LqParseErrorCode::LimitExceededNfa);
    }

    #[test]
    fn lookahead_is_forbidden_syntax() {
        // regex-syntax rejects lookaround at parse time; the guard
        // re-maps that to ForbiddenSyntax per the RFC.
        assert_eq!(err_code("(?=foo)"), LqParseErrorCode::ForbiddenSyntax);
    }

    #[test]
    fn lookbehind_is_forbidden_syntax() {
        assert_eq!(err_code("(?<=foo)"), LqParseErrorCode::ForbiddenSyntax);
    }

    #[test]
    fn backreference_is_forbidden_syntax() {
        // `\1` is a backreference; regex-syntax rejects it as
        // UnsupportedBackreference which we map to ForbiddenSyntax.
        assert_eq!(err_code(r"(foo)\1"), LqParseErrorCode::ForbiddenSyntax);
    }

    #[test]
    fn malformed_regex_is_regex_parse() {
        assert_eq!(err_code("a{,"), LqParseErrorCode::RegexParse);
    }
}
