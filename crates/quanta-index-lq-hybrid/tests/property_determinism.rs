//! Proptest: hybrid executor is byte-identical-deterministic.
//!
//! For any random `(lex_candidates, sem_candidates, top_k)` triple, running
//! [`HybridExecutor::execute`] twice on the same input must yield the same
//! fused sequence — including the `HybridContribution.fused_score` values,
//! the merge-tuple ordering, and the truncation length.

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_hybrid::{
    CandidateRef, DocId, FusionStrategy, HybridContribution, HybridExecutor, LexCandidate,
    ManifestGeneration, RepoId, SemCandidate,
};

fn arb_cref() -> impl Strategy<Value = CandidateRef> {
    (
        0_u64..16_u64, // doc_id (small to force overlap)
        0_u64..4_u64,  // repo_id
        prop_oneof![
            Just("a".to_owned()),
            Just("b".to_owned()),
            Just("c".to_owned())
        ],
        0_u32..8_u32,
    )
        .prop_map(|(doc, repo, path, line)| CandidateRef {
            doc_id: DocId(doc),
            repo_id: RepoId(repo),
            // Pin a single generation across all candidates so the
            // executor's generation check does not fire — we are
            // testing the fusion+merge determinism, not the generation
            // pin path (covered by unit tests).
            generation: ManifestGeneration(1),
            repo_relative_path: path.into_boxed_str(),
            start_line: line,
        })
}

fn arb_lex() -> impl Strategy<Value = LexCandidate> {
    (arb_cref(), 1_u32..32_u32, 0.0_f32..50.0_f32).prop_map(|(c, rank, score)| LexCandidate {
        candidate_ref: c,
        rank,
        score,
    })
}

fn arb_sem() -> impl Strategy<Value = SemCandidate> {
    (arb_cref(), 1_u32..32_u32, 0.0_f32..1.0_f32).prop_map(|(c, rank, score)| SemCandidate {
        candidate_ref: c,
        rank,
        score,
    })
}

fn arb_strategy() -> impl Strategy<Value = FusionStrategy> {
    prop_oneof![
        (1_u32..256_u32).prop_map(|k| FusionStrategy::Rrf { k }),
        (0.001_f32..2.0_f32, 0.001_f32..2.0_f32).prop_map(|(l, s)| FusionStrategy::Weighted {
            lex_weight: l,
            sem_weight: s,
        }),
    ]
}

fn ids_and_scores(v: &[HybridContribution]) -> Vec<(u64, u32)> {
    // Use the IEEE bit-pattern of `fused_score` for byte-identical
    // comparison (deterministic across runs because the algorithm is
    // pure-functional over its inputs).
    v.iter()
        .map(|r| (r.candidate_ref.doc_id.0, r.fused_score.to_bits()))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn execute_twice_yields_identical_sequence(
        lex_in in vec(arb_lex(), 0..16),
        sem_in in vec(arb_sem(), 0..16),
        strategy in arb_strategy(),
        top_k in 0_u32..64_u32,
    ) {
        let exec = HybridExecutor::new(strategy);
        let a = exec.execute(&lex_in, &sem_in, top_k);
        let b = exec.execute(&lex_in, &sem_in, top_k);
        match (a, b) {
            (Ok(av), Ok(bv)) => {
                prop_assert_eq!(ids_and_scores(&av), ids_and_scores(&bv));
            }
            (Err(ae), Err(be)) => {
                prop_assert_eq!(ae.code, be.code);
            }
            (a, b) => {
                prop_assert!(false, "ok/err disagreement: {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn execute_truncates_to_top_k_bound(
        lex_in in vec(arb_lex(), 0..16),
        sem_in in vec(arb_sem(), 0..16),
        top_k in 0_u32..64_u32,
    ) {
        let exec = HybridExecutor::new(FusionStrategy::Rrf { k: 60 });
        let out = exec.execute(&lex_in, &sem_in, top_k);
        if let Ok(v) = out {
            // `top_k` is bounded by the proptest range (< 64) so `usize::try_from`
            // is total on every 32-bit-or-wider platform. If it ever failed
            // (impossible on supported targets), the empty-cap fallback would
            // surface the issue as a property failure rather than a panic.
            let Ok(cap) = usize::try_from(top_k) else {
                prop_assert!(false, "usize::try_from(top_k) failed for {top_k}");
                return Ok(());
            };
            prop_assert!(v.len() <= cap);
        }
    }
}
