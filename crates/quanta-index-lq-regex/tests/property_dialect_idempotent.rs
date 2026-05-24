//! Property test: every pattern that `regex_syntax::parse` accepts also
//! passes [`quanta_index_lq_regex::dialect_filter`] cleanly.
//!
//! The shrunk corpus is bounded to 256 cases per ticket guidance.

use proptest::prelude::*;
use quanta_index_lq_regex::dialect_filter;

/// Strategy producing RE2-valid (or RE2-invalid; we filter) patterns
/// from a small alphabet. Keeps the search space tight so 256 cases run
/// in <1s.
fn pattern_strategy() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("a".to_string()),
            Just("b".to_string()),
            Just("z".to_string()),
            Just(".".to_string()),
            Just(r"\w".to_string()),
            Just(r"\s".to_string()),
            Just(r"\b".to_string()),
            Just(r"\d".to_string()),
            Just("^".to_string()),
            Just("$".to_string()),
            Just("|".to_string()),
            Just("[a-z]".to_string()),
            Just("[0-9]".to_string()),
            Just("(a|b)".to_string()),
            Just("a*".to_string()),
            Just("a+".to_string()),
            Just("a?".to_string()),
            Just("a{1,3}".to_string()),
        ],
        1..6,
    )
    .prop_map(|parts| parts.join(""))
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// For every pattern `regex_syntax::parse` accepts, the dialect
    /// filter accepts the resulting HIR. This is the v1 invariant: the
    /// dialect filter is a no-op for everything `regex_syntax` accepts.
    #[test]
    fn parse_implies_dialect_ok(p in pattern_strategy()) {
        if let Ok(hir) = regex_syntax::parse(&p) {
            // `regex_syntax::parse` succeeded → dialect filter must not
            // reject (v1 invariant).
            let r = dialect_filter(&hir);
            prop_assert!(r.is_ok(), "dialect filter rejected accepted pattern {p:?}: {r:?}");
        }
    }
}
