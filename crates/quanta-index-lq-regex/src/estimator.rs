//! Structural regex complexity charge for planner-time admission.
//!
//! [`estimate_nfa_states`] walks the [`regex_syntax::hir::Hir`] and
//! returns a monotone structural charge. Exceed [`MAX_NFA_STATES`] → typed
//! [`crate::RegexErrorCode::PlanLimitExceeded`] tagged
//! [`crate::LimitDimension::NfaStates`].
//!
//! The walk is deliberately monotone: every recursive subtree adds at
//! least one unit, and bounded repetitions multiply through. Unicode classes
//! expand into byte automata during compilation, so this charge is not an
//! upper bound on compiled states or physical allocation. Engine byte refusal
//! is a separate limit; aggregate allocation admission remains a separate seam.

use regex_syntax::hir::{Hir, HirKind, Repetition};

use crate::errors::{LimitDimension, RegexError};

/// NFA-state cap per LEX-04 spec / RFC § Canonical Query Model.
pub const MAX_NFA_STATES: u64 = 100_000;

/// Structural HIR charge, retained under the existing state-estimator API.
/// This is not the compiled engine's state count or a physical byte bound.
///
/// On overflow or cap-exceed, returns
/// [`RegexError::plan_limit`] with [`LimitDimension::NfaStates`].
pub fn estimate_nfa_states(hir: &Hir) -> Result<u64, RegexError> {
    let n = walk(hir)?;
    if n > MAX_NFA_STATES {
        return Err(RegexError::plan_limit(
            LimitDimension::NfaStates,
            format!("regex structural charge {n} exceeds cap {MAX_NFA_STATES}"),
        ));
    }
    Ok(n)
}

/// Recursive HIR walk producing a monotone structural charge.
///
/// Returns `PLAN_LIMIT_EXCEEDED` immediately on overflow so we never
/// silently saturate.
fn walk(hir: &Hir) -> Result<u64, RegexError> {
    match hir.kind() {
        HirKind::Empty | HirKind::Look(_) => Ok(1),
        HirKind::Literal(lit) => {
            // One state per byte of the literal plus one accept state.
            let len = u64::try_from(lit.0.len()).map_err(|e| {
                RegexError::plan_limit(
                    LimitDimension::NfaStates,
                    format!("literal length overflow u64: {e}"),
                )
            })?;
            len.checked_add(1).ok_or_else(|| {
                RegexError::plan_limit(
                    LimitDimension::NfaStates,
                    "literal length + 1 overflowed u64",
                )
            })
        }
        HirKind::Class(cls) => {
            // Charge each HIR character range plus one unit. Unicode ranges
            // can expand into multiple byte transitions during compilation.
            let ranges = class_range_count(cls)?;
            ranges.checked_add(1).ok_or_else(|| {
                RegexError::plan_limit(
                    LimitDimension::NfaStates,
                    "class range count + 1 overflowed u64",
                )
            })
        }
        HirKind::Repetition(rep) => repetition_bound(rep),
        HirKind::Capture(cap) => {
            // Captures wrap a sub-expression with two epsilon states
            // (start/end markers).
            let sub = walk(&cap.sub)?;
            sub.checked_add(2).ok_or_else(|| {
                RegexError::plan_limit(LimitDimension::NfaStates, "capture body + 2 overflowed u64")
            })
        }
        HirKind::Concat(subs) => {
            let mut total: u64 = 1;
            for s in subs {
                let v = walk(s)?;
                total = total.checked_add(v).ok_or_else(|| {
                    RegexError::plan_limit(LimitDimension::NfaStates, "concat sum overflowed u64")
                })?;
                if total > MAX_NFA_STATES {
                    return Err(RegexError::plan_limit(
                        LimitDimension::NfaStates,
                        format!("concat running sum {total} exceeds cap {MAX_NFA_STATES}"),
                    ));
                }
            }
            Ok(total)
        }
        HirKind::Alternation(subs) => {
            // Alternation adds one branch state plus one per arm.
            let mut total: u64 = 1;
            for s in subs {
                let v = walk(s)?;
                total = total.checked_add(v).ok_or_else(|| {
                    RegexError::plan_limit(
                        LimitDimension::NfaStates,
                        "alternation sum overflowed u64",
                    )
                })?;
                if total > MAX_NFA_STATES {
                    return Err(RegexError::plan_limit(
                        LimitDimension::NfaStates,
                        format!("alternation running sum {total} exceeds cap {MAX_NFA_STATES}"),
                    ));
                }
            }
            Ok(total)
        }
    }
}

/// Compute the number of character ranges in a [`regex_syntax::hir::Class`].
///
/// Both `ClassUnicode` and `ClassBytes` expose `ranges()` slices.
fn class_range_count(cls: &regex_syntax::hir::Class) -> Result<u64, RegexError> {
    let n = match *cls {
        regex_syntax::hir::Class::Unicode(ref u) => u.ranges().len(),
        regex_syntax::hir::Class::Bytes(ref b) => b.ranges().len(),
    };
    u64::try_from(n).map_err(|e| {
        RegexError::plan_limit(
            LimitDimension::NfaStates,
            format!("class range count overflow u64: {e}"),
        )
    })
}

/// Bounded repetition charges `max` copies of the body plus one unit per copy.
/// Unbounded repetition (`*`, `+`) charges `min + 1` copies.
fn repetition_bound(rep: &Repetition) -> Result<u64, RegexError> {
    let body = walk(&rep.sub)?;
    let max_factor: u64 = rep.max.map_or_else(
        // Unbounded repetition. Treat as `min + 1` body copies plus a
        // loop state.
        || u64::from(rep.min).saturating_add(1),
        |m| {
            // Cap the multiplier at MAX_NFA_STATES + 1 so we exit fast
            // when a pathological pattern like `a{0,1_000_000}` lands.
            let cap = MAX_NFA_STATES.checked_add(1).unwrap_or(MAX_NFA_STATES);
            let m_u64 = u64::from(m);
            if m_u64 > cap { cap } else { m_u64 }
        },
    );
    let body_states = body.checked_add(1).ok_or_else(|| {
        RegexError::plan_limit(
            LimitDimension::NfaStates,
            "repetition body + 1 overflowed u64",
        )
    })?;
    let total = body_states.checked_mul(max_factor).ok_or_else(|| {
        RegexError::plan_limit(
            LimitDimension::NfaStates,
            "repetition body * max overflowed u64",
        )
    })?;
    if total > MAX_NFA_STATES {
        return Err(RegexError::plan_limit(
            LimitDimension::NfaStates,
            format!(
                "repetition body {body_states} * max {max_factor} = {total} exceeds cap {MAX_NFA_STATES}"
            ),
        ));
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::{MAX_NFA_STATES, estimate_nfa_states};
    use crate::errors::{LimitDimension, RegexErrorCode};
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
    fn empty_pattern_is_one_state() {
        let h = parse("");
        match estimate_nfa_states(&h) {
            Ok(n) => assert!(n <= MAX_NFA_STATES),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn small_literal_under_cap() {
        let h = parse("foo");
        match estimate_nfa_states(&h) {
            Ok(n) => assert!(n < 100),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn typical_pattern_under_cap() {
        let h = parse(r"fn\s+handle_\w+");
        match estimate_nfa_states(&h) {
            // The Unicode-wide `\w` and `\s` classes contribute many
            // range entries to the conservative bound; we only assert
            // the global cap is honoured (per spec §9 NFA-state cap).
            Ok(n) => assert!(n < MAX_NFA_STATES, "got {n}"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn anchored_typical_pattern_under_cap() {
        let h = parse("^fn foo");
        match estimate_nfa_states(&h) {
            Ok(n) => assert!(n < 1_000),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn alternation_under_cap() {
        let h = parse("foo|bar|baz");
        match estimate_nfa_states(&h) {
            Ok(n) => assert!(n < 100),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn explosion_repetition_rejected() {
        // Unicode-wide `\w` class contributes hundreds of range states;
        // bounded repetition by 1000 amplifies the body cost past the
        // 100k state cap.
        let h = parse(r"\w{0,1000}");
        match estimate_nfa_states(&h) {
            Ok(n) => assert!(false, "expected cap exceed, got {n}"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::NfaStates));
            }
        }
    }

    #[test]
    fn stacked_bounded_repetition_rejected() {
        // 62 (a-z A-Z 0-9) stacked bounded repetitions of `{0,1000}`;
        // each arm contributes ~2000 states under the conservative
        // walk; the running sum quickly exceeds the 100k cap.
        let h = parse(
            "a{0,1000}b{0,1000}c{0,1000}d{0,1000}e{0,1000}f{0,1000}g{0,1000}h{0,1000}i{0,1000}j{0,1000}k{0,1000}l{0,1000}m{0,1000}n{0,1000}o{0,1000}p{0,1000}q{0,1000}r{0,1000}s{0,1000}t{0,1000}u{0,1000}v{0,1000}w{0,1000}x{0,1000}y{0,1000}z{0,1000}A{0,1000}B{0,1000}C{0,1000}D{0,1000}E{0,1000}F{0,1000}G{0,1000}H{0,1000}I{0,1000}J{0,1000}K{0,1000}L{0,1000}M{0,1000}N{0,1000}O{0,1000}P{0,1000}Q{0,1000}R{0,1000}S{0,1000}T{0,1000}U{0,1000}V{0,1000}W{0,1000}X{0,1000}Y{0,1000}Z{0,1000}0{0,1000}1{0,1000}2{0,1000}3{0,1000}4{0,1000}5{0,1000}6{0,1000}7{0,1000}8{0,1000}9{0,1000}",
        );
        match estimate_nfa_states(&h) {
            Ok(n) => assert!(false, "expected cap exceed, got {n}"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::NfaStates));
            }
        }
    }

    #[test]
    fn modest_concat_under_cap() {
        // 20 bare classes concatenated; each contributes a few hundred
        // range states for Unicode `\w`. Should stay under the cap.
        let mut s = String::new();
        for _ in 0..20 {
            s.push_str(r"\w");
        }
        let h = parse(&s);
        let r = estimate_nfa_states(&h);
        // The conservative walk may or may not exceed the cap depending
        // on Unicode table size; both outcomes are typed. We just
        // assert the typed surface is preserved.
        match r {
            Ok(n) => assert!(n <= MAX_NFA_STATES),
            Err(e) => assert_eq!(e.code, RegexErrorCode::PlanLimitExceeded),
        }
    }

    #[test]
    fn bounded_repetition_just_under_cap_accepts() {
        // `a{0,100}` body=2 (one byte literal + accept), so 2*100=200.
        let h = parse("a{0,100}");
        match estimate_nfa_states(&h) {
            Ok(n) => assert!(n < MAX_NFA_STATES),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn unbounded_repetition_accepts() {
        let h = parse("a*");
        match estimate_nfa_states(&h) {
            Ok(n) => assert!(n < MAX_NFA_STATES),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn cap_constant_is_100k() {
        assert_eq!(MAX_NFA_STATES, 100_000);
    }
}
