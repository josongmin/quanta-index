//! Reciprocal Rank Fusion (default per SEM-02 §4.4).
//!
//! ## Algorithm
//!
//! For each unique candidate `c` observed across the lexical and semantic
//! sub-result lists, the fused score is:
//!
//! ```text
//! fused(c) = sum over engines e ∈ {lex, sem} where c ∈ top_k_e:
//!                1.0 / (k + rank_e(c))
//! ```
//!
//! where `rank_e(c)` is the 1-indexed rank from engine `e` and `k` is the
//! RRF rank constant ([`crate::RRF_DEFAULT_K`] = 60 by literature standard).
//!
//! The fusion is **scale-free** — it consumes only `(candidate_ref, rank)`
//! tuples from each engine, never the raw lex/sem scores. The raw scores
//! are nonetheless threaded through into the [`HybridContribution`] so
//! that downstream merge (§4.5) can use them as tie-breakers and so that
//! callers see them in the explanation envelope.
//!
//! ## Determinism
//!
//! The input lists may arrive in any order; the implementation indexes
//! candidates by `CandidateRef` equality and accumulates contributions
//! in a way that depends only on the contents of the input lists, not on
//! their order at the per-engine level. The output vector order is
//! **not specified** by this function — apply
//! [`crate::order_by_merge_tuple`] before exposing results to a caller.

use crate::contribution::{HybridContribution, LexCandidate, SemCandidate};
use crate::types::CandidateRef;

/// Reciprocal Rank Fusion over lexical + semantic candidate lists.
///
/// Returns one [`HybridContribution`] per unique [`CandidateRef`]
/// observed. Ordering of the returned vector is not pinned here —
/// apply [`crate::order_by_merge_tuple`] before exposing the result.
#[must_use]
pub fn fuse_rrf(lex: &[LexCandidate], sem: &[SemCandidate], k: u32) -> Vec<HybridContribution> {
    // Collect canonical refs in first-seen order. Duplicates within one
    // engine's list keep the first occurrence (engines are contracted to
    // deliver unique refs per top-k; defensive against accidental drift).
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
        let fused = rrf_value(lex_rank, sem_rank, k);
        out.push(HybridContribution {
            candidate_ref: cref,
            lex_rank,
            lex_score,
            sem_rank,
            sem_score,
            fused_score: fused,
        });
    }
    out
}

fn position(refs: &[CandidateRef], probe: &CandidateRef) -> Option<usize> {
    for (i, r) in refs.iter().enumerate() {
        if r == probe {
            return Some(i);
        }
    }
    None
}

fn rrf_term(rank: u32, k: u32) -> f32 {
    // `k + rank` is bounded by `2 * u32::MAX` (saturating). For RRF
    // values the denom is realistically `≤ 60 + 10_000` per SEM-02 §3.1,
    // well below `2^14`, so the `u32 -> f32` conversion is exact. We use
    // `u16::try_from` fallback chains to keep the conversion clippy-clean
    // without invoking the workspace-banned `unwrap_or*` family.
    let denom = u64::from(k).saturating_add(u64::from(rank));
    let denom_f = u64_to_f32(denom);
    if denom_f <= 0.0_f32 {
        // Unreachable: rank ≥ 1 by contract from each engine's top-k
        // (1-indexed) and k ≥ 0, so `denom ≥ 1` always. Defensive 0.0
        // preserves additive identity; never panics.
        return 0.0_f32;
    }
    1.0_f32 / denom_f
}

#[expect(
    clippy::option_if_let_else,
    reason = "explicit match keeps the typed-failure shape visible at the conversion boundary"
)]
fn u64_to_f32(v: u64) -> f32 {
    // For values that fit in u16, `f32::from(u16)` is exact and lint-clean.
    // Above u16 we fall through to a string-parse narrowing (matches the
    // sibling SEM-01 cosine kernel approach; performance is negligible
    // because RRF denoms are bounded by SEM-02 §3.1 well below u16::MAX).
    match u16::try_from(v) {
        Ok(small) => f32::from(small),
        Err(_) => narrow_u64_to_f32(v),
    }
}

#[expect(
    clippy::manual_unwrap_or,
    reason = "workspace clippy.toml disallows Result::unwrap_or; the explicit match is the typed escape hatch"
)]
#[expect(
    clippy::option_if_let_else,
    reason = "explicit match keeps the typed-failure shape visible at the narrow boundary"
)]
fn narrow_u64_to_f32(v: u64) -> f32 {
    // String-parse narrowing path. Format produces a finite-decimal
    // representation for every u64; `f32::from_str` returns `Ok(Inf)`
    // for values that overflow f32 (≥ 2^128) but every u64 is bounded
    // by `< 2^64 < 2^128`, so the result is always finite. We return
    // an explicit match arm rather than `unwrap_or*` to comply with the
    // workspace `disallowed-methods` policy.
    let s = format!("{v}");
    match s.parse::<f32>() {
        Ok(f) => f,
        // Unreachable: `f32::from_str` accepts every integer formatted
        // via `{}`. 0.0 keeps determinism without panic.
        Err(_) => 0.0_f32,
    }
}

fn rrf_value(lex_rank: Option<u32>, sem_rank: Option<u32>, k: u32) -> f32 {
    let lex_part = lex_rank.map_or(0.0_f32, |r| rrf_term(r, k));
    let sem_part = sem_rank.map_or(0.0_f32, |r| rrf_term(r, k));
    // Sum of two values in `[0, 1]` is finite; well within f32 range.
    lex_part + sem_part
}

#[cfg(test)]
mod tests {
    use super::{fuse_rrf, rrf_term, rrf_value};
    use crate::contribution::{LexCandidate, SemCandidate};
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

    #[test]
    fn rrf_term_known_value() {
        let t = rrf_term(1, 60);
        let expected = 1.0_f32 / 61.0_f32;
        assert!(
            (t - expected).abs() < 1.0e-6,
            "got {t}, expected {expected}"
        );
    }

    #[test]
    fn rrf_value_both_sides() {
        let v = rrf_value(Some(1), Some(2), 60);
        let expected = 1.0_f32 / 61.0_f32 + 1.0_f32 / 62.0_f32;
        assert!((v - expected).abs() < 1.0e-6, "got {v}");
    }

    #[test]
    fn rrf_value_lex_only() {
        let v = rrf_value(Some(1), None, 60);
        let expected = 1.0_f32 / 61.0_f32;
        assert!((v - expected).abs() < 1.0e-6, "got {v}");
    }

    #[test]
    fn rrf_value_sem_only() {
        let v = rrf_value(None, Some(1), 60);
        let expected = 1.0_f32 / 61.0_f32;
        assert!((v - expected).abs() < 1.0e-6, "got {v}");
    }

    #[test]
    fn fuse_produces_one_row_per_unique_ref() {
        let l = vec![lex(1, 1, 10.0), lex(2, 2, 9.0)];
        let s = vec![sem(2, 1, 0.9), sem(3, 2, 0.8)];
        let out = fuse_rrf(&l, &s, 60);
        assert_eq!(out.len(), 3, "expected 3 unique refs, got {}", out.len());
    }

    #[test]
    fn fuse_both_sides_outranks_one_side_at_same_rank() {
        let l = vec![lex(1, 1, 10.0), lex(2, 1, 10.0)];
        let s = vec![sem(1, 1, 0.9), sem(3, 1, 0.9)];
        let out = fuse_rrf(&l, &s, 60);
        let mut doc1_score = 0.0_f32;
        let mut doc2_score = 0.0_f32;
        let mut doc3_score = 0.0_f32;
        for c in &out {
            if c.candidate_ref.doc_id.0 == 1 {
                doc1_score = c.fused_score;
            } else if c.candidate_ref.doc_id.0 == 2 {
                doc2_score = c.fused_score;
            } else if c.candidate_ref.doc_id.0 == 3 {
                doc3_score = c.fused_score;
            }
        }
        assert!(
            doc1_score > doc2_score,
            "doc1 ({doc1_score}) should outrank doc2 ({doc2_score})"
        );
        assert!(
            doc1_score > doc3_score,
            "doc1 ({doc1_score}) should outrank doc3 ({doc3_score})"
        );
    }

    #[test]
    fn fuse_lex_score_threaded_into_contribution() {
        let l = vec![lex(1, 1, 12.5)];
        let s: Vec<SemCandidate> = vec![];
        let out = fuse_rrf(&l, &s, 60);
        assert_eq!(out.len(), 1);
        let Some(row) = out.first() else {
            assert!(false, "missing row");
            return;
        };
        assert_eq!(row.lex_score, Some(12.5));
        assert_eq!(row.lex_rank, Some(1));
        assert_eq!(row.sem_score, None);
        assert_eq!(row.sem_rank, None);
    }

    #[test]
    fn fuse_empty_inputs_yields_empty_output() {
        let l: Vec<LexCandidate> = vec![];
        let s: Vec<SemCandidate> = vec![];
        let out = fuse_rrf(&l, &s, 60);
        assert!(out.is_empty());
    }

    #[test]
    fn fuse_worked_example_from_spec() {
        // SEM-02 §4.6 worked example, unweighted (caller composes weights
        // by choosing the Weighted strategy if they want score-blending;
        // this is the canonical scale-free RRF surface).
        let l = vec![lex(1, 1, 12.5), lex(2, 2, 11.0), lex(3, 3, 9.8)];
        let s = vec![sem(4, 1, 0.92), sem(1, 2, 0.88), sem(5, 3, 0.85)];
        let out = fuse_rrf(&l, &s, 60);
        let expected_a = 1.0_f32 / 61.0_f32 + 1.0_f32 / 62.0_f32;
        let expected_b = 1.0_f32 / 62.0_f32;
        let expected_c = 1.0_f32 / 63.0_f32;
        let expected_d = 1.0_f32 / 61.0_f32;
        let expected_e = 1.0_f32 / 63.0_f32;
        for c in &out {
            match c.candidate_ref.doc_id.0 {
                1 => assert!(
                    (c.fused_score - expected_a).abs() < 1.0e-6,
                    "A got {}",
                    c.fused_score
                ),
                2 => assert!(
                    (c.fused_score - expected_b).abs() < 1.0e-6,
                    "B got {}",
                    c.fused_score
                ),
                3 => assert!(
                    (c.fused_score - expected_c).abs() < 1.0e-6,
                    "C got {}",
                    c.fused_score
                ),
                4 => assert!(
                    (c.fused_score - expected_d).abs() < 1.0e-6,
                    "D got {}",
                    c.fused_score
                ),
                5 => assert!(
                    (c.fused_score - expected_e).abs() < 1.0e-6,
                    "E got {}",
                    c.fused_score
                ),
                other => assert!(false, "unexpected doc {other}"),
            }
        }
    }
}
