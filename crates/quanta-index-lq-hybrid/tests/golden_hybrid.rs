//! Golden ordering for the SEM-02 hybrid executor.
//!
//! Five lexical candidates × five semantic candidates with deliberate
//! overlap. Both fusion strategies (RRF default, Weighted opt-in) are
//! exercised; the fused top-k ordering is pinned per strategy.

use quanta_index_lq_hybrid::{
    CandidateRef, DocId, FusionStrategy, HybridExecutor, LexCandidate, ManifestGeneration, RepoId,
    SemCandidate,
};

fn fatal(msg: &str) -> ! {
    assert!(false, "{msg}");
    std::process::abort();
}

fn cref(doc: u64) -> CandidateRef {
    CandidateRef {
        doc_id: DocId(doc),
        repo_id: RepoId(1),
        generation: ManifestGeneration(1),
        repo_relative_path: Box::<str>::from(format!("src/d{doc}.rs")),
        start_line: 1,
    }
}

fn lex(doc: u64, rank: u32, score: f32) -> LexCandidate {
    LexCandidate {
        candidate_ref: cref(doc),
        rank,
        score,
    }
}

fn sem(doc: u64, rank: u32, score: f32) -> SemCandidate {
    SemCandidate {
        candidate_ref: cref(doc),
        rank,
        score,
    }
}

fn ids(v: &[quanta_index_lq_hybrid::HybridContribution]) -> Vec<u64> {
    v.iter().map(|r| r.candidate_ref.doc_id.0).collect()
}

/// Five lex candidates, five sem candidates. Two overlap (doc 1, 3).
fn build_lex() -> Vec<LexCandidate> {
    vec![
        lex(1, 1, 12.5),
        lex(2, 2, 11.0),
        lex(3, 3, 9.8),
        lex(6, 4, 8.5),
        lex(7, 5, 7.0),
    ]
}

fn build_sem() -> Vec<SemCandidate> {
    vec![
        sem(4, 1, 0.92),
        sem(1, 2, 0.88),
        sem(3, 3, 0.85),
        sem(5, 4, 0.80),
        sem(8, 5, 0.75),
    ]
}

#[test]
fn rrf_default_pins_top5_ordering() {
    let exec = HybridExecutor::new(FusionStrategy::Rrf { k: 60 });
    let out = match exec.execute(&build_lex(), &build_sem(), 5) {
        Ok(v) => v,
        Err(e) => fatal(&format!("execute: {e}")),
    };
    // RRF unweighted scores (k = 60):
    //   doc 1: 1/61 + 1/62 = ~0.03245   ← both sides
    //   doc 3: 1/63 + 1/63 = ~0.03175   ← both sides
    //   doc 4: 1/61         = 0.01639   ← sem only
    //   doc 2: 1/62         = 0.01613   ← lex only
    //   doc 5: 1/64         = 0.01563   ← sem only
    //   doc 6: 1/64         = 0.01563   ← lex only (tied with 5)
    //   doc 7: 1/65         = 0.01538   ← lex only
    //   doc 8: 1/65         = 0.01538   ← sem only (tied with 7)
    //
    // top-5 fused order: 1, 3, 4, 2, then tie between 5 and 6 at ~0.01563.
    // tier 2: lex_score DESC NULL_LAST — doc 6 has lex_score Some, doc 5
    // has lex_score None → 6 sorts before 5.
    let id_seq = ids(&out);
    assert_eq!(id_seq.len(), 5);
    assert_eq!(
        id_seq,
        vec![1, 3, 4, 2, 6],
        "expected top-5 sequence pinned by RRF + 8-tuple merge"
    );
}

#[test]
fn weighted_strategy_pins_top3_ordering() {
    let exec = HybridExecutor::new(FusionStrategy::Weighted {
        lex_weight: 0.5,
        sem_weight: 0.5,
    });
    let out = match exec.execute(&build_lex(), &build_sem(), 3) {
        Ok(v) => v,
        Err(e) => fatal(&format!("execute: {e}")),
    };
    // Weighted scores (0.5 lex + 0.5 sem):
    //   doc 1: 0.5*12.5 + 0.5*0.88 = 6.69   ← both sides
    //   doc 2: 0.5*11.0           = 5.50
    //   doc 3: 0.5*9.8  + 0.5*0.85 = 5.325  ← both sides
    //   doc 6: 0.5*8.5            = 4.25
    //   doc 7: 0.5*7.0            = 3.50
    //   doc 4: 0.5*0.92           = 0.46
    //   doc 5: 0.5*0.80           = 0.40
    //   doc 8: 0.5*0.75           = 0.375
    //
    // top-3: doc 1, doc 2, doc 3.
    let id_seq = ids(&out);
    assert_eq!(id_seq, vec![1, 2, 3]);
}

#[test]
fn rrf_strategy_top_k_one_picks_overlap_doc() {
    let exec = HybridExecutor::new(FusionStrategy::Rrf { k: 60 });
    let out = match exec.execute(&build_lex(), &build_sem(), 1) {
        Ok(v) => v,
        Err(e) => fatal(&format!("execute: {e}")),
    };
    assert_eq!(out.len(), 1);
    let Some(r0) = out.first() else {
        fatal("missing top-1");
    };
    assert_eq!(r0.candidate_ref.doc_id.0, 1);
}

#[test]
fn rrf_strategy_full_output_is_eight_unique() {
    let exec = HybridExecutor::new(FusionStrategy::Rrf { k: 60 });
    let out = match exec.execute(&build_lex(), &build_sem(), 100) {
        Ok(v) => v,
        Err(e) => fatal(&format!("execute: {e}")),
    };
    // 5 lex + 5 sem with 2 overlaps → 8 unique candidates.
    assert_eq!(out.len(), 8);
    let mut id_seq = ids(&out);
    id_seq.sort_unstable();
    assert_eq!(id_seq, vec![1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn hybrid_contribution_carries_both_ranks_for_overlap() {
    let exec = HybridExecutor::new(FusionStrategy::Rrf { k: 60 });
    let out = match exec.execute(&build_lex(), &build_sem(), 100) {
        Ok(v) => v,
        Err(e) => fatal(&format!("execute: {e}")),
    };
    for row in &out {
        match row.candidate_ref.doc_id.0 {
            1 => {
                assert_eq!(row.lex_rank, Some(1));
                assert_eq!(row.sem_rank, Some(2));
                assert!(row.lex_score.is_some());
                assert!(row.sem_score.is_some());
            }
            3 => {
                assert_eq!(row.lex_rank, Some(3));
                assert_eq!(row.sem_rank, Some(3));
            }
            2 | 6 | 7 => {
                assert!(row.lex_rank.is_some());
                assert!(row.sem_rank.is_none());
            }
            4 | 5 | 8 => {
                assert!(row.lex_rank.is_none());
                assert!(row.sem_rank.is_some());
            }
            other => fatal(&format!("unexpected doc {other}")),
        }
    }
}

#[test]
fn rrf_default_strategy_deterministic_top5() {
    // Repeated execution → byte-identical fused sequence.
    let exec = HybridExecutor::new(FusionStrategy::Rrf { k: 60 });
    let l = build_lex();
    let s = build_sem();
    let a = match exec.execute(&l, &s, 5) {
        Ok(v) => v,
        Err(e) => fatal(&format!("a: {e}")),
    };
    let b = match exec.execute(&l, &s, 5) {
        Ok(v) => v,
        Err(e) => fatal(&format!("b: {e}")),
    };
    assert_eq!(ids(&a), ids(&b));
}
