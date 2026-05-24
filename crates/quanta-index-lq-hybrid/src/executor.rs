//! Hybrid executor — wires strategy → fuse → merge → truncate.
//!
//! Per SEM-02 §4.1–§4.5, the executor:
//!
//! 1. validates `top_k` against [`MAX_TOP_K`] (`10_000`);
//! 2. checks generation pinning: every candidate from both sides must
//!    share the same `ManifestGeneration` — mismatch → `HYB_GEN_MISMATCH`
//!    (fail-closed per §3.4);
//! 3. dispatches to the chosen [`FusionStrategy`] (RRF or `WeightedScore`);
//! 4. orders the fused vector via [`crate::order_by_merge_tuple`];
//! 5. truncates to `top_k`.
//!
//! `top_k == 0` returns an empty result rather than an error per the
//! spec's "blocked, typed" discipline (§Step 9 of the ticket: typed-error
//! discipline says zero-result is a valid `empty` shape, not a failure).
//! `top_k > MAX_TOP_K` returns `HYB_TOP_K_INVALID`.

use crate::contribution::{HybridContribution, LexCandidate, SemCandidate};
use crate::errors::{HybridError, HybridErrorCode};
use crate::merge::order_by_merge_tuple;
use crate::rrf::fuse_rrf;
use crate::strategy::{FusionStrategy, HybridWeights};
use crate::types::ManifestGeneration;
use crate::weighted::fuse_weighted;

/// Top-k ceiling per SEM-02 §3.1 / DSL §13.
pub const MAX_TOP_K: u32 = 10_000;

/// Pinned-strategy hybrid executor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HybridExecutor {
    strategy: FusionStrategy,
}

impl HybridExecutor {
    /// Construct an executor for a given fusion strategy.
    #[must_use]
    pub const fn new(strategy: FusionStrategy) -> Self {
        Self { strategy }
    }

    /// Borrow the pinned strategy.
    #[must_use]
    pub const fn strategy(&self) -> &FusionStrategy {
        &self.strategy
    }

    /// Execute fusion end-to-end.
    ///
    /// Returns the fused rows ordered by the SEM-02 §4.5 merge tuple,
    /// truncated to `top_k`. Returns:
    ///
    /// - empty vector if `top_k == 0` (the "empty" result shape is
    ///   valid; see SEM-02 §4.6 / UC-HYB-14);
    /// - `HYB_TOP_K_INVALID` if `top_k > MAX_TOP_K`;
    /// - `HYB_GEN_MISMATCH` if lex and sem candidates carry different
    ///   `ManifestGeneration` values;
    /// - whatever the underlying strategy returns (e.g.
    ///   `HYB_SUBQUERY_INVALID` for non-finite scores under Weighted).
    pub fn execute(
        &self,
        lex: &[LexCandidate],
        sem: &[SemCandidate],
        top_k: u32,
    ) -> Result<Vec<HybridContribution>, HybridError> {
        if top_k > MAX_TOP_K {
            return Err(HybridError::new(
                HybridErrorCode::HybTopKInvalid,
                format!("top_k {top_k} exceeds MAX_TOP_K {MAX_TOP_K}"),
            ));
        }
        if top_k == 0 {
            return Ok(Vec::new());
        }

        check_generation_pin(lex, sem)?;

        let fused = match self.strategy {
            FusionStrategy::Rrf { k } => fuse_rrf(lex, sem, k),
            FusionStrategy::Weighted {
                lex_weight,
                sem_weight,
            } => {
                let w = HybridWeights::new(lex_weight, sem_weight)?;
                fuse_weighted(lex, sem, w)?
            }
        };

        let ordered = order_by_merge_tuple(fused);
        let truncated = truncate_to(ordered, top_k);
        Ok(truncated)
    }
}

fn check_generation_pin(lex: &[LexCandidate], sem: &[SemCandidate]) -> Result<(), HybridError> {
    let mut pin: Option<ManifestGeneration> = None;
    for c in lex {
        match pin {
            None => pin = Some(c.candidate_ref.generation),
            Some(p) if p == c.candidate_ref.generation => {}
            Some(p) => {
                return Err(HybridError::new(
                    HybridErrorCode::HybGenMismatch,
                    format!(
                        "lex candidate doc {} pins generation {} but executor expected {}",
                        c.candidate_ref.doc_id, c.candidate_ref.generation, p,
                    ),
                ));
            }
        }
    }
    for c in sem {
        match pin {
            None => pin = Some(c.candidate_ref.generation),
            Some(p) if p == c.candidate_ref.generation => {}
            Some(p) => {
                return Err(HybridError::new(
                    HybridErrorCode::HybGenMismatch,
                    format!(
                        "sem candidate doc {} pins generation {} but executor expected {}",
                        c.candidate_ref.doc_id, c.candidate_ref.generation, p,
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn truncate_to(mut v: Vec<HybridContribution>, top_k: u32) -> Vec<HybridContribution> {
    // `top_k <= MAX_TOP_K = 10_000`; usize is at least 16-bit per Rust
    // target spec which guarantees fit. The `map_or` defensive fallback
    // keeps the full vector if the conversion ever fails (it cannot at
    // the spec-bound).
    let cap = usize::try_from(top_k).map_or(v.len(), |x| x);
    if v.len() > cap {
        v.truncate(cap);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::{HybridExecutor, MAX_TOP_K};
    use crate::contribution::{LexCandidate, SemCandidate};
    use crate::errors::HybridErrorCode;
    use crate::strategy::FusionStrategy;
    use crate::types::{CandidateRef, DocId, ManifestGeneration, RepoId};

    fn cref(doc: u64, gen_: u64) -> CandidateRef {
        CandidateRef {
            doc_id: DocId(doc),
            repo_id: RepoId(1),
            generation: ManifestGeneration(gen_),
            repo_relative_path: Box::<str>::from("p"),
            start_line: 1,
        }
    }

    fn lex(doc: u64, gen_: u64, rank: u32, score: f32) -> LexCandidate {
        LexCandidate {
            candidate_ref: cref(doc, gen_),
            rank,
            score,
        }
    }

    fn sem(doc: u64, gen_: u64, rank: u32, score: f32) -> SemCandidate {
        SemCandidate {
            candidate_ref: cref(doc, gen_),
            rank,
            score,
        }
    }

    fn exec_rrf() -> HybridExecutor {
        HybridExecutor::new(FusionStrategy::Rrf { k: 60 })
    }

    #[test]
    fn execute_top_k_zero_returns_empty() {
        let e = exec_rrf();
        let out = match e.execute(&[lex(1, 1, 1, 10.0)], &[], 0) {
            Ok(v) => v,
            Err(err) => {
                assert!(false, "{err}");
                return;
            }
        };
        assert!(out.is_empty());
    }

    #[test]
    fn execute_top_k_above_max_errors() {
        let e = exec_rrf();
        match e.execute(&[], &[], MAX_TOP_K.saturating_add(1)) {
            Ok(_) => assert!(false, "expected HYB_TOP_K_INVALID"),
            Err(err) => assert_eq!(err.code, HybridErrorCode::HybTopKInvalid),
        }
    }

    #[test]
    fn execute_top_k_at_max_ok() {
        let e = exec_rrf();
        match e.execute(&[], &[], MAX_TOP_K) {
            Ok(out) => assert!(out.is_empty()),
            Err(err) => assert!(false, "{err}"),
        }
    }

    #[test]
    fn execute_truncates_to_top_k() {
        let e = exec_rrf();
        let l = vec![
            lex(1, 1, 1, 10.0),
            lex(2, 1, 2, 9.0),
            lex(3, 1, 3, 8.0),
            lex(4, 1, 4, 7.0),
            lex(5, 1, 5, 6.0),
        ];
        let s: Vec<SemCandidate> = vec![];
        let out = match e.execute(&l, &s, 2) {
            Ok(v) => v,
            Err(err) => {
                assert!(false, "{err}");
                return;
            }
        };
        assert_eq!(out.len(), 2);
        // After merge tuple: lex top-rank wins.
        let Some(r0) = out.first() else {
            assert!(false, "missing rank 0");
            return;
        };
        let Some(r1) = out.get(1) else {
            assert!(false, "missing rank 1");
            return;
        };
        assert_eq!(r0.candidate_ref.doc_id.0, 1);
        assert_eq!(r1.candidate_ref.doc_id.0, 2);
    }

    #[test]
    fn execute_generation_mismatch_lex_vs_sem_errors() {
        let e = exec_rrf();
        let l = vec![lex(1, 1, 1, 10.0)];
        let s = vec![sem(2, 2, 1, 0.9)]; // generation 2 vs lex generation 1
        match e.execute(&l, &s, 5) {
            Ok(_) => assert!(false, "expected HYB_GEN_MISMATCH"),
            Err(err) => assert_eq!(err.code, HybridErrorCode::HybGenMismatch),
        }
    }

    #[test]
    fn execute_generation_mismatch_within_lex_errors() {
        let e = exec_rrf();
        let l = vec![lex(1, 1, 1, 10.0), lex(2, 2, 2, 9.0)];
        let s: Vec<SemCandidate> = vec![];
        match e.execute(&l, &s, 5) {
            Ok(_) => assert!(false, "expected HYB_GEN_MISMATCH"),
            Err(err) => assert_eq!(err.code, HybridErrorCode::HybGenMismatch),
        }
    }

    #[test]
    fn execute_generation_mismatch_within_sem_errors() {
        let e = exec_rrf();
        let l: Vec<LexCandidate> = vec![];
        let s = vec![sem(1, 1, 1, 0.9), sem(2, 2, 2, 0.8)];
        match e.execute(&l, &s, 5) {
            Ok(_) => assert!(false, "expected HYB_GEN_MISMATCH"),
            Err(err) => assert_eq!(err.code, HybridErrorCode::HybGenMismatch),
        }
    }

    #[test]
    fn execute_weighted_invalid_weights_in_strategy_errors() {
        let e = HybridExecutor::new(FusionStrategy::Weighted {
            lex_weight: -0.1,
            sem_weight: 0.5,
        });
        match e.execute(&[lex(1, 1, 1, 10.0)], &[], 5) {
            Ok(_) => assert!(false, "expected HYB_INVALID_WEIGHTS"),
            Err(err) => assert_eq!(err.code, HybridErrorCode::HybInvalidWeights),
        }
    }

    #[test]
    fn execute_weighted_path_returns_blended_scores() {
        let e = HybridExecutor::new(FusionStrategy::Weighted {
            lex_weight: 0.5,
            sem_weight: 0.5,
        });
        let l = vec![lex(1, 1, 1, 10.0)];
        let s = vec![sem(1, 1, 1, 1.0)];
        let out = match e.execute(&l, &s, 5) {
            Ok(v) => v,
            Err(err) => {
                assert!(false, "{err}");
                return;
            }
        };
        assert_eq!(out.len(), 1);
        let expected = 0.5_f32.mul_add(10.0_f32, 0.5_f32 * 1.0_f32);
        let Some(row) = out.first() else {
            assert!(false, "missing row");
            return;
        };
        assert!((row.fused_score - expected).abs() < 1.0e-6);
    }

    #[test]
    fn execute_empty_inputs_top_k_5_returns_empty() {
        let e = exec_rrf();
        let out = match e.execute(&[], &[], 5) {
            Ok(v) => v,
            Err(err) => {
                assert!(false, "{err}");
                return;
            }
        };
        assert!(out.is_empty());
    }

    #[test]
    fn strategy_accessor() {
        let e = exec_rrf();
        match e.strategy() {
            FusionStrategy::Rrf { k } => assert_eq!(*k, 60),
            FusionStrategy::Weighted { .. } => assert!(false, "wrong strategy"),
        }
    }
}
