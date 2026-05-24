//! Proptest: RRF monotonicity — both-sides candidates dominate one-side
//! candidates at the same rank.
//!
//! For any random rank `r`, an "in-both-engines at rank r" candidate
//! produces `fused = 2/(k+r)` while a "single-engine at rank r" candidate
//! produces `fused = 1/(k+r)`. Therefore the dual-sided candidate's
//! `fused_score` must strictly exceed the single-sided candidate's.
//!
//! Also asserts: ranks closer to 1 outrank ranks farther from 1 on the
//! same engine (basic ordering property — defensive against accidental
//! sign flip).

use proptest::prelude::*;

use quanta_index_lq_hybrid::{
    CandidateRef, DocId, FusionStrategy, HybridExecutor, LexCandidate, ManifestGeneration, RepoId,
    SemCandidate, fuse_rrf,
};

fn cref(doc: u64) -> CandidateRef {
    CandidateRef {
        doc_id: DocId(doc),
        repo_id: RepoId(1),
        generation: ManifestGeneration(1),
        repo_relative_path: Box::<str>::from("p"),
        start_line: 1,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn both_sides_outranks_one_side_at_equal_rank(
        rank in 1_u32..50_u32,
        k in 1_u32..200_u32,
    ) {
        // Three candidates, all at the same rank within their respective engine:
        //   doc 1: in lex AND sem (both at `rank`)
        //   doc 2: in lex only (at `rank`)
        //   doc 3: in sem only (at `rank`)
        let lex = vec![
            LexCandidate { candidate_ref: cref(1), rank, score: 1.0 },
            LexCandidate { candidate_ref: cref(2), rank, score: 1.0 },
        ];
        let sem = vec![
            SemCandidate { candidate_ref: cref(1), rank, score: 0.5 },
            SemCandidate { candidate_ref: cref(3), rank, score: 0.5 },
        ];
        let out = fuse_rrf(&lex, &sem, k);
        let mut doc1 = None;
        let mut doc2 = None;
        let mut doc3 = None;
        for c in &out {
            match c.candidate_ref.doc_id.0 {
                1 => doc1 = Some(c.fused_score),
                2 => doc2 = Some(c.fused_score),
                3 => doc3 = Some(c.fused_score),
                other => prop_assert!(false, "unexpected doc {other}"),
            }
        }
        let (Some(d1), Some(d2), Some(d3)) = (doc1, doc2, doc3) else {
            prop_assert!(false, "missing doc");
            return Ok(());
        };
        prop_assert!(d1 > d2, "both-sides ({d1}) should beat lex-only ({d2})");
        prop_assert!(d1 > d3, "both-sides ({d1}) should beat sem-only ({d3})");
    }

    #[test]
    fn lower_rank_outranks_higher_rank_on_same_engine(
        better_rank in 1_u32..16_u32,
        offset in 1_u32..16_u32,
        k in 1_u32..200_u32,
    ) {
        let worse_rank = better_rank.saturating_add(offset);
        let lex = vec![
            LexCandidate { candidate_ref: cref(1), rank: better_rank, score: 1.0 },
            LexCandidate { candidate_ref: cref(2), rank: worse_rank, score: 1.0 },
        ];
        let sem: Vec<SemCandidate> = vec![];
        let out = fuse_rrf(&lex, &sem, k);
        let mut doc1 = None;
        let mut doc2 = None;
        for c in &out {
            if c.candidate_ref.doc_id.0 == 1 {
                doc1 = Some(c.fused_score);
            } else if c.candidate_ref.doc_id.0 == 2 {
                doc2 = Some(c.fused_score);
            }
        }
        let (Some(d1), Some(d2)) = (doc1, doc2) else {
            prop_assert!(false, "missing doc");
            return Ok(());
        };
        prop_assert!(d1 > d2, "rank {better_rank} should beat rank {worse_rank}");
    }

    #[test]
    fn dual_sided_top_rank_dominates_executor_top_k(
        k in 1_u32..200_u32,
    ) {
        // Doc 1 is in BOTH engines at rank 1.
        // Doc 2 is only in lex at rank 1.
        // Doc 3 is only in sem at rank 1.
        // After executor merge, doc 1 must be at rank 1 of the fused output.
        let lex = vec![
            LexCandidate { candidate_ref: cref(1), rank: 1, score: 1.0 },
            LexCandidate { candidate_ref: cref(2), rank: 1, score: 1.0 },
        ];
        let sem = vec![
            SemCandidate { candidate_ref: cref(1), rank: 1, score: 1.0 },
            SemCandidate { candidate_ref: cref(3), rank: 1, score: 1.0 },
        ];
        let exec = HybridExecutor::new(FusionStrategy::Rrf { k });
        let out = match exec.execute(&lex, &sem, 3) {
            Ok(v) => v,
            Err(e) => {
                prop_assert!(false, "execute: {e}");
                return Ok(());
            }
        };
        prop_assert_eq!(out.len(), 3);
        let Some(r0) = out.first() else {
            prop_assert!(false, "missing top-1");
            return Ok(());
        };
        prop_assert_eq!(r0.candidate_ref.doc_id.0, 1);
    }
}
