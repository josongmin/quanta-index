//! Proptest: [`CompositeScorer::explain`] sum invariant.
//!
//! For arbitrary valid [`CandidateSignals`] paired with arbitrary valid
//! [`RankerWeights`], the sum of [`SignalContribution::contribution`]
//! from [`CompositeScorer::explain`] must match the pre-clamp raw
//! weighted sum within `1e-5` f32 epsilon, and the returned
//! `clamped_score` must equal [`CompositeScorer::score`] for the same
//! input.
//!
//! Two-axis property — joint sweep over weights and signals — so a
//! single regression in `compute_contributions`, `sum_contributions`,
//! or `finalize_score` surfaces here regardless of which axis triggers
//! it.

use proptest::prelude::*;

use quanta_index_lq_ranker::{
    CandidateSignals, CompositeScorer, RankerWeights, SignalContribution,
};

/// Construct a valid `RankerWeights` from five `[0.0, 1.0]` weights by
/// projecting onto the unit-sum simplex.
///
/// `RankerWeights::new` rejects sums outside `1.0 ± 1e-3`; we therefore
/// normalize by dividing by the actual sum. If every input is zero we
/// fall back to `DEFAULTS` so we always emit a valid weight set.
///
/// Returns `None` only if normalization produced a quintuple that
/// `RankerWeights::new` rejects (e.g. floating-point drift outside the
/// tolerance). The proptest harness retries on `None` via
/// `prop_filter_map`.
fn weights_from_unit_quintuple(
    bm25: f32,
    path_prior: f32,
    symbol_boost: f32,
    recency: f32,
    boost: f32,
) -> Option<RankerWeights> {
    let sum = bm25 + path_prior + symbol_boost + recency + boost;
    if sum < 1e-3 {
        return Some(RankerWeights::DEFAULTS);
    }
    // We deliberately do not propagate the typed error here: the only
    // call site is the proptest filter, which treats rejection as
    // "regenerate". The contract is "valid weights or None".
    let built = RankerWeights::new(
        bm25 / sum,
        path_prior / sum,
        symbol_boost / sum,
        recency / sum,
        boost / sum,
    );
    if let Ok(w) = built {
        return Some(w);
    }
    None
}

fn arb_weights() -> impl Strategy<Value = RankerWeights> {
    (
        0.01_f32..=1.0_f32,
        0.01_f32..=1.0_f32,
        0.01_f32..=1.0_f32,
        0.01_f32..=1.0_f32,
        0.01_f32..=1.0_f32,
    )
        .prop_filter_map(
            "must validate under RankerWeights::new",
            |(bm25, path_prior, symbol_boost, recency, boost)| {
                weights_from_unit_quintuple(bm25, path_prior, symbol_boost, recency, boost)
            },
        )
}

fn arb_signals() -> impl Strategy<Value = CandidateSignals> {
    (
        0.0_f32..=1.0_f32,
        0.0_f32..=1.0_f32,
        0.0_f32..=1.0_f32,
        0.0_f32..=1.0_f32,
        0.125_f32..=8.0_f32,
    )
        .prop_map(|(bm25, pp, sb, rec, boost)| CandidateSignals {
            bm25,
            path_prior: pp,
            symbol_boost: sb,
            recency: rec,
            boost_directive: boost,
        })
}

fn pre_clamp_raw(contribs: &[SignalContribution]) -> f32 {
    let mut acc: f32 = 0.0;
    for c in contribs {
        acc += c.contribution;
    }
    acc
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn explain_contribution_sum_matches_raw_within_epsilon(
        w in arb_weights(),
        sig in arb_signals(),
    ) {
        let scorer = match CompositeScorer::new(w) {
            Ok(s) => s,
            Err(e) => {
                prop_assert!(false, "scorer construction failed: {e}");
                return Ok(());
            }
        };
        let exp = match scorer.explain(&sig) {
            Ok(v) => v,
            Err(e) => {
                prop_assert!(false, "explain failed: {e}");
                return Ok(());
            }
        };
        let raw_from_contribs = pre_clamp_raw(&exp.contributions);
        // Reproduce the same five products + four adds the scorer performs,
        // accumulated into a `Vec` and folded — keeps the test sum
        // bit-equal to `sum_contributions` without tripping
        // `clippy::suboptimal_flops` (no horizontal `a*b + c*d + ...`
        // chain that clippy could rewrite into a `mul_add` cascade).
        let products: Vec<f32> = vec![
            w.bm25() * sig.bm25,
            w.path_prior() * sig.path_prior,
            w.symbol_boost() * sig.symbol_boost,
            w.recency() * sig.recency,
            w.boost_directive() * (sig.boost_directive - 1.0),
        ];
        let mut raw_via_score_function: f32 = 0.0;
        for p in &products {
            raw_via_score_function += *p;
        }
        // The two raw sums should match within f32 epsilon.
        prop_assert!(
            (raw_from_contribs - raw_via_score_function).abs() <= 1.0e-5,
            "contribution sum {raw_from_contribs} diverged from raw {raw_via_score_function}",
        );
    }

    #[test]
    fn explanation_clamped_score_equals_score(
        w in arb_weights(),
        sig in arb_signals(),
    ) {
        let scorer = match CompositeScorer::new(w) {
            Ok(s) => s,
            Err(e) => {
                prop_assert!(false, "scorer construction failed: {e}");
                return Ok(());
            }
        };
        let score_only = match scorer.score(&sig) {
            Ok(v) => v,
            Err(e) => {
                prop_assert!(false, "score failed: {e}");
                return Ok(());
            }
        };
        let explained = match scorer.explain(&sig) {
            Ok(v) => v,
            Err(e) => {
                prop_assert!(false, "explain failed: {e}");
                return Ok(());
            }
        };
        // Bit-equal: both paths fold the same five products through the
        // same `finalize_score` clamp.
        prop_assert_eq!(score_only.to_bits(), explained.clamped_score.to_bits());
    }
}
