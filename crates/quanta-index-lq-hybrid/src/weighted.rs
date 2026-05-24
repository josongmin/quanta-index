//! `WeightedScore` fusion strategy (opt-in per SEM-02 §4.4).
//!
//! ## Algorithm
//!
//! For each unique candidate `c` observed across the lexical and semantic
//! sub-result lists, the fused score is:
//!
//! ```text
//! fused(c) = w.lex * lex_score(c) + w.sem * sem_score(c)
//! ```
//!
//! Missing-side contributions are treated as zero: a candidate present
//! only on the lex side scores `w.lex * lex_score`. The result is the
//! one engine's score weighted by the corresponding weight.
//!
//! ## Failure modes
//!
//! Any `NaN` or `Inf` observed in `lex_score` / `sem_score` returns
//! `HYB_SUBQUERY_INVALID`. Weight validity is checked at
//! [`HybridWeights::new`] before reaching this function.

use crate::contribution::{HybridContribution, LexCandidate, SemCandidate};
use crate::errors::{HybridError, HybridErrorCode};
use crate::strategy::HybridWeights;
use crate::types::CandidateRef;

/// `WeightedScore` fusion over lexical + semantic candidate lists.
///
/// Returns one [`HybridContribution`] per unique [`CandidateRef`]
/// observed. Ordering of the returned vector is not pinned here —
/// apply [`crate::order_by_merge_tuple`] before exposing the result.
pub fn fuse_weighted(
    lex: &[LexCandidate],
    sem: &[SemCandidate],
    w: HybridWeights,
) -> Result<Vec<HybridContribution>, HybridError> {
    // Validate finiteness on all sub-query scores up-front (no silent
    // NaN propagation).
    for (i, c) in lex.iter().enumerate() {
        if !c.score.is_finite() {
            return Err(HybridError::new(
                HybridErrorCode::HybSubqueryInvalid,
                format!("lex candidate {i} score is non-finite: {}", c.score),
            ));
        }
    }
    for (j, c) in sem.iter().enumerate() {
        if !c.score.is_finite() {
            return Err(HybridError::new(
                HybridErrorCode::HybSubqueryInvalid,
                format!("sem candidate {j} score is non-finite: {}", c.score),
            ));
        }
    }

    let mut refs: Vec<CandidateRef> = Vec::new();
    let mut lex_idx: Vec<Option<usize>> = Vec::new();
    let mut sem_idx: Vec<Option<usize>> = Vec::new();

    for (i, c) in lex.iter().enumerate() {
        if position(&refs, &c.candidate_ref).is_none() {
            refs.push(c.candidate_ref.clone());
            lex_idx.push(Some(i));
            sem_idx.push(None);
        }
    }
    for (j, c) in sem.iter().enumerate() {
        if let Some(p) = position(&refs, &c.candidate_ref) {
            if let Some(slot) = sem_idx.get_mut(p)
                && slot.is_none()
            {
                *slot = Some(j);
            }
        } else {
            refs.push(c.candidate_ref.clone());
            lex_idx.push(None);
            sem_idx.push(Some(j));
        }
    }

    let mut out: Vec<HybridContribution> = Vec::with_capacity(refs.len());
    for (i, cref) in refs.into_iter().enumerate() {
        let lex_hit = lex_idx.get(i).copied().flatten().and_then(|li| lex.get(li));
        let sem_hit = sem_idx.get(i).copied().flatten().and_then(|si| sem.get(si));
        let lex_rank = lex_hit.map(|c| c.rank);
        let lex_score = lex_hit.map(|c| c.score);
        let sem_rank = sem_hit.map(|c| c.rank);
        let sem_score = sem_hit.map(|c| c.score);
        let fused = weighted_value(lex_score, sem_score, w);
        if !fused.is_finite() {
            return Err(HybridError::new(
                HybridErrorCode::HybSubqueryInvalid,
                format!("weighted fused score for doc {} is non-finite", cref.doc_id),
            ));
        }
        out.push(HybridContribution {
            candidate_ref: cref,
            lex_rank,
            lex_score,
            sem_rank,
            sem_score,
            fused_score: fused,
        });
    }
    Ok(out)
}

fn position(refs: &[CandidateRef], probe: &CandidateRef) -> Option<usize> {
    for (i, r) in refs.iter().enumerate() {
        if r == probe {
            return Some(i);
        }
    }
    None
}

fn weighted_value(lex_score: Option<f32>, sem_score: Option<f32>, w: HybridWeights) -> f32 {
    // Closed linear form; weights are bounded finite by `HybridWeights::new`
    // and scores are bounded finite by the validate loop in `fuse_weighted`.
    let lex_part = lex_score.map_or(0.0_f32, |s| w.lex() * s);
    let sem_part = sem_score.map_or(0.0_f32, |s| w.sem() * s);
    lex_part + sem_part
}

#[cfg(test)]
mod tests {
    use super::fuse_weighted;
    use crate::contribution::{LexCandidate, SemCandidate};
    use crate::errors::HybridErrorCode;
    use crate::strategy::HybridWeights;
    use crate::types::{CandidateRef, DocId, ManifestGeneration, RepoId};

    fn cref(doc: u64) -> CandidateRef {
        CandidateRef {
            doc_id: DocId(doc),
            repo_id: RepoId(1),
            generation: ManifestGeneration(1),
            repo_relative_path: Box::<str>::from("p"),
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

    fn w(l: f32, s: f32) -> HybridWeights {
        match HybridWeights::new(l, s) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                HybridWeights::DEFAULTS
            }
        }
    }

    #[test]
    fn weighted_both_sides_combines() {
        let l = vec![lex(1, 1, 12.0)];
        let s = vec![sem(1, 1, 0.8)];
        let out = match fuse_weighted(&l, &s, w(0.5, 0.5)) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(out.len(), 1);
        let Some(row) = out.first() else {
            assert!(false, "missing row");
            return;
        };
        let expected = 0.5_f32.mul_add(12.0_f32, 0.5_f32 * 0.8_f32);
        assert!((row.fused_score - expected).abs() < 1.0e-6);
    }

    #[test]
    fn weighted_lex_only() {
        let l = vec![lex(1, 1, 10.0)];
        let s: Vec<SemCandidate> = vec![];
        let out = match fuse_weighted(&l, &s, w(0.7, 0.3)) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(out.len(), 1);
        let Some(row) = out.first() else {
            assert!(false, "missing row");
            return;
        };
        let expected = 0.7_f32 * 10.0_f32;
        assert!((row.fused_score - expected).abs() < 1.0e-6);
    }

    #[test]
    fn weighted_sem_only() {
        let l: Vec<LexCandidate> = vec![];
        let s = vec![sem(1, 1, 0.9)];
        let out = match fuse_weighted(&l, &s, w(0.4, 0.6)) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(out.len(), 1);
        let Some(row) = out.first() else {
            assert!(false, "missing row");
            return;
        };
        let expected = 0.6_f32 * 0.9_f32;
        assert!((row.fused_score - expected).abs() < 1.0e-6);
    }

    #[test]
    fn weighted_rejects_nan_lex_score() {
        let l = vec![lex(1, 1, f32::NAN)];
        let s: Vec<SemCandidate> = vec![];
        match fuse_weighted(&l, &s, w(0.5, 0.5)) {
            Ok(_) => assert!(false, "expected HYB_SUBQUERY_INVALID"),
            Err(e) => assert_eq!(e.code, HybridErrorCode::HybSubqueryInvalid),
        }
    }

    #[test]
    fn weighted_rejects_inf_sem_score() {
        let l: Vec<LexCandidate> = vec![];
        let s = vec![sem(1, 1, f32::INFINITY)];
        match fuse_weighted(&l, &s, w(0.5, 0.5)) {
            Ok(_) => assert!(false, "expected HYB_SUBQUERY_INVALID"),
            Err(e) => assert_eq!(e.code, HybridErrorCode::HybSubqueryInvalid),
        }
    }

    #[test]
    fn weighted_empty_inputs_yields_empty_output() {
        let l: Vec<LexCandidate> = vec![];
        let s: Vec<SemCandidate> = vec![];
        let out = match fuse_weighted(&l, &s, w(0.5, 0.5)) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert!(out.is_empty());
    }
}
