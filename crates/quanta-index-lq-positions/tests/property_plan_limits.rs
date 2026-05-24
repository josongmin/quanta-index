//! Property tests — `PLAN_LIMIT_EXCEEDED` cap invariants for LEX-03 §4.2.
//!
//! Two properties:
//!
//! 1. For any phrase length `n > MAX_PHRASE_LEN`, `query_phrase` must
//!    return `Err(PlanLimitExceeded { dimension: PhraseLen })`.
//! 2. For any builder run whose per-`(term, doc)` cell observation count
//!    stays strictly below `MAX_POSITIONS_PER_CELL`, every `add_token`
//!    must succeed (the cap must not trigger under-budget inputs).
//!
//! These are correctness rails for the cap surface, complementing the
//! per-cap unit tests in `src/{builder,phrase_query,adjacency_query}.rs`.

use proptest::prelude::*;

use quanta_index_lq_positions::{
    DocId, LimitDimension, MAX_PHRASE_LEN, MAX_POSITIONS_PER_CELL, NormalizerVersion, Position,
    PositionsBuilder, PositionsErrorCode, PositionsIndex, query_phrase,
};

#[expect(
    clippy::unreachable,
    clippy::option_if_let_else,
    reason = "fixture: an empty builder has no caps to trip; the panic-via-unreachable arm is dead by construction and serves only to surface a regression"
)]
fn empty_index() -> PositionsIndex {
    let b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
    // The empty builder has no terms; `finish` walks zero entries and
    // cannot trip any cap. An `Err` here would indicate a regression in
    // the builder itself rather than a property-test failure.
    match b.finish() {
        Ok(v) => v,
        Err(_) => unreachable!("empty builder must finish"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn phrase_length_over_cap_always_plan_limit_exceeded(
        // Cap+1 .. cap+64; bounded so prop runs cheap. The test index is
        // empty so the cap check is the only thing that can run.
        over_by in 1u32..=64u32,
    ) {
        let idx = empty_index();
        let n_u32 = MAX_PHRASE_LEN.saturating_add(over_by);
        let Ok(n_usize) = usize::try_from(n_u32) else {
            return Err(TestCaseError::reject("usize conv".to_owned()));
        };
        let terms: Vec<&str> = vec!["x"; n_usize];
        match query_phrase(&idx, &terms) {
            Ok(_) => prop_assert!(false, "expected PlanLimitExceeded for n={n_u32}"),
            Err(e) => {
                prop_assert_eq!(e.code, PositionsErrorCode::PlanLimitExceeded);
                prop_assert_eq!(e.dimension, Some(LimitDimension::PhraseLen));
            }
        }
    }

    #[test]
    fn add_token_under_cell_cap_always_ok(
        // n_under is the count of `add_token` calls for a single
        // (term, doc) cell, strictly below MAX_POSITIONS_PER_CELL.
        n_under in 0u32..MAX_POSITIONS_PER_CELL,
        // Use a few distinct doc ids / terms to exercise the per-cell
        // isolation without inflating proptest cost.
        doc in 0u64..=8u64,
        term_idx in 0usize..4usize,
    ) {
        let terms = ["a", "the", "fn", "_"];
        let term = match terms.get(term_idx) {
            Some(t) => *t,
            None => return Err(TestCaseError::reject("term_idx".to_owned())),
        };
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        for i in 0..n_under {
            match b.add_token(DocId(doc), term, Position(i)) {
                Ok(()) => {}
                Err(e) => prop_assert!(
                    false,
                    "add_token must succeed under cap (i={i}, n_under={n_under}): {e}"
                ),
            }
        }
        // Finalisation must also succeed — DocsPerTerm cap is one term ≤ 1.
        prop_assert!(b.finish().is_ok());
    }

    #[test]
    fn phrase_length_at_or_under_cap_never_returns_phrase_len_error(
        // 0..=cap inclusive — the boundary itself must be accepted.
        n in 0u32..=MAX_PHRASE_LEN,
    ) {
        let idx = empty_index();
        let Ok(n_usize) = usize::try_from(n) else {
            return Err(TestCaseError::reject("usize conv".to_owned()));
        };
        let terms: Vec<&str> = vec!["x"; n_usize];
        match query_phrase(&idx, &terms) {
            // Result set against the empty index is empty; that's the
            // only ok outcome we permit. Cross-cap errors would also be
            // fine for non-cap codes but the empty fixture eliminates
            // those paths.
            Ok(r) => prop_assert!(r.matches.is_empty()),
            Err(e) => {
                let msg = format!("PhraseLen must not trigger at n={n}");
                prop_assert_ne!(e.dimension, Some(LimitDimension::PhraseLen), "{}", msg);
            }
        }
    }
}
