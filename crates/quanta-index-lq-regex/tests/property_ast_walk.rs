//! Property tests for [`quanta_index_lq_regex::ast_walk_filter`].
//!
//! Two invariants are exercised:
//!
//! 1. **Clean patterns** — patterns built from a "safe" alphabet that
//!    contains no possessive `(?>` token, no `\k<…>` escape, and no
//!    non-leading `(?<flag>)` set-flag → AST walk returns `Ok(())`.
//! 2. **Injected possessive** — for any non-empty prefix `P`, the pattern
//!    `P(?>a)` is rejected with [`ForbiddenKind::Possessive`]. This
//!    confirms the detection uses absolute pattern offsets, not relative
//!    to start.
//!
//! Cases are bounded to 256 per the project's property-test budget.

use proptest::prelude::*;
use quanta_index_lq_regex::{ForbiddenKind, RegexErrorCode, ast_walk_filter};

/// Build a "safe" pattern fragment. None of these tokens contain a
/// possessive `(?>`, a `\k<…>`, or any inline-flag set-flag form.
fn safe_fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("a".to_string()),
        Just("b".to_string()),
        Just("z".to_string()),
        Just(r"\w".to_string()),
        Just(r"\s".to_string()),
        Just(r"\b".to_string()),
        Just(r"\d".to_string()),
        Just("[a-z]".to_string()),
        Just("(a|b)".to_string()),
        Just("(?:foo)".to_string()),
        Just("a*".to_string()),
        Just("a+".to_string()),
    ]
}

fn safe_pattern() -> impl Strategy<Value = String> {
    proptest::collection::vec(safe_fragment(), 1..6).prop_map(|parts| parts.join(""))
}

/// Pick a non-empty "safe" prefix to put before an injected `(?>a)`. We
/// require non-empty because the empty-prefix case is exercised in the
/// unit tests.
fn safe_prefix() -> impl Strategy<Value = String> {
    proptest::collection::vec(safe_fragment(), 1..4).prop_map(|parts| parts.join(""))
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// Safe patterns are accepted by the AST walk.
    ///
    /// Only patterns that `regex_syntax::ast::Parser::parse` actually
    /// accepts are checked; if the random join produces an unparseable
    /// shape (e.g. `(a*` from a corner case), we skip that case.
    #[test]
    fn safe_patterns_accepted(p in safe_pattern()) {
        if regex_syntax::ast::parse::Parser::new().parse(&p).is_ok() {
            let r = ast_walk_filter(&p);
            prop_assert!(r.is_ok(), "AST walk rejected safe pattern {p:?}: {r:?}");
        }
    }

    /// Injecting `(?>a)` after any safe prefix triggers
    /// `ForbiddenKind::Possessive` detection — never a different
    /// `ForbiddenKind` and never silent acceptance.
    #[test]
    fn injected_possessive_is_caught(prefix in safe_prefix()) {
        let pattern = format!("{prefix}(?>a)");
        match ast_walk_filter(&pattern) {
            Ok(()) => prop_assert!(
                false,
                "AST walk silently accepted possessive in {pattern:?}"
            ),
            Err(e) => {
                prop_assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                prop_assert_eq!(e.forbidden, Some(ForbiddenKind::Possessive));
            }
        }
    }
}
