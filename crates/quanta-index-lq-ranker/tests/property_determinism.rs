//! Proptest: [`rank_candidates`] is deterministic.
//!
//! For any random [`ScoredCandidate`] vector, sorting twice through
//! [`rank_candidates`] must produce the same `doc_id` sequence on both
//! passes. The tiebreak key is total over `ScoredCandidate`, so there is
//! no input shape (including full ties on every tier other than `doc_id`)
//! that should yield two different stable orderings.
//!
//! Also asserts that the sort is idempotent: re-sorting an already-sorted
//! vector preserves the order. This catches accidental reverse-step
//! oscillation in the tiebreak `cmp` impl.

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_ranker::{CandidateSignals, ScoredCandidate, rank_candidates};

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

fn arb_path() -> impl Strategy<Value = String> {
    // Small fixed alphabet of 8 paths to encourage path-tier ties.
    prop_oneof![
        Just("a".to_owned()),
        Just("ab".to_owned()),
        Just("ac".to_owned()),
        Just("b".to_owned()),
        Just("ba".to_owned()),
        Just("c".to_owned()),
        Just("ca".to_owned()),
        Just("d".to_owned()),
    ]
}

fn arb_candidate() -> impl Strategy<Value = ScoredCandidate> {
    (
        // doc_id: keep small so we can force tier-1/2/3/4/5 ties.
        0_u64..32_u64,
        // repo_id, generation: also small to encourage ties.
        0_u64..8_u64,
        0_u64..8_u64,
        arb_path(),
        0_u32..8_u32,
        // score: clamp to the post-clamp envelope `[0.0, 1.0]`.
        0.0_f32..=1.0_f32,
        arb_signals(),
    )
        .prop_map(
            |(doc_id, repo_id, generation, path, start_line, score, signals)| ScoredCandidate {
                doc_id,
                repo_id,
                generation,
                repo_relative_path: path.into_boxed_str(),
                start_line,
                score,
                signals,
            },
        )
}

fn ids(v: &[ScoredCandidate]) -> Vec<u64> {
    v.iter().map(|c| c.doc_id).collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn sort_twice_yields_identical_doc_id_sequence(
        v in vec(arb_candidate(), 0..32),
    ) {
        let a = rank_candidates(v.clone());
        let b = rank_candidates(v);
        prop_assert_eq!(ids(&a), ids(&b));
    }

    #[test]
    fn sort_is_idempotent(
        v in vec(arb_candidate(), 0..32),
    ) {
        let once = rank_candidates(v);
        let twice = rank_candidates(once.clone());
        prop_assert_eq!(ids(&once), ids(&twice));
    }

    #[test]
    fn sort_is_permutation_of_input(
        v in vec(arb_candidate(), 0..32),
    ) {
        let mut input_ids = ids(&v);
        let sorted = rank_candidates(v);
        let mut output_ids = ids(&sorted);
        input_ids.sort_unstable();
        output_ids.sort_unstable();
        prop_assert_eq!(input_ids, output_ids);
    }
}
