//! Extended merge-determinism tuple sort for hybrid fusion (SEM-02 §4.5).
//!
//! The fused merge order is total via the 8-component tuple:
//!
//! ```text
//! 1. fused_score           DESC                    (primary key)
//! 2. lex_score             DESC, `NULL_LAST`
//! 3. sem_score             DESC, `NULL_LAST`
//! 4. repo_id               ASC
//! 5. manifest_generation   ASC
//! 6. repo_relative_path    ASC (byte-lex)
//! 7. start_line            ASC
//! 8. candidate_id (doc_id) ASC
//! ```
//!
//! `f32` keys are compared via [`f32::total_cmp`] so NaN-shape inputs
//! still order deterministically. `Option<f32>` keys treat `None` as
//! greater than any `Some` (`NULL_LAST` under descending order is `None`
//! comes after every concrete score).
//!
//! The sort is **stable**; combined with the 8-component total tuple,
//! there are no observable ties and the output sequence is bit-exact
//! reproducible across instances per SEM-02 §5.7.

use core::cmp::Ordering;

use crate::contribution::HybridContribution;

/// Sort `v` into hybrid output order via the SEM-02 §4.5 8-component tuple.
///
/// Stable sort: equal-key inputs preserve their relative input order
/// (though equality is unreachable because `doc_id` makes the tuple
/// total).
#[must_use]
pub fn order_by_merge_tuple(mut v: Vec<HybridContribution>) -> Vec<HybridContribution> {
    v.sort_by(cmp_merge_tuple);
    v
}

fn cmp_merge_tuple(a: &HybridContribution, b: &HybridContribution) -> Ordering {
    // Tier 1: fused_score DESC.
    match b.fused_score.total_cmp(&a.fused_score) {
        Ordering::Equal => {}
        non_eq @ (Ordering::Less | Ordering::Greater) => return non_eq,
    }
    // Tier 2: lex_score DESC, `NULL_LAST`.
    match cmp_opt_f32_desc_null_last(a.lex_score, b.lex_score) {
        Ordering::Equal => {}
        non_eq @ (Ordering::Less | Ordering::Greater) => return non_eq,
    }
    // Tier 3: sem_score DESC, `NULL_LAST`.
    match cmp_opt_f32_desc_null_last(a.sem_score, b.sem_score) {
        Ordering::Equal => {}
        non_eq @ (Ordering::Less | Ordering::Greater) => return non_eq,
    }
    // Tier 4: repo_id ASC.
    match a.candidate_ref.repo_id.0.cmp(&b.candidate_ref.repo_id.0) {
        Ordering::Equal => {}
        non_eq @ (Ordering::Less | Ordering::Greater) => return non_eq,
    }
    // Tier 5: manifest_generation ASC.
    match a
        .candidate_ref
        .generation
        .0
        .cmp(&b.candidate_ref.generation.0)
    {
        Ordering::Equal => {}
        non_eq @ (Ordering::Less | Ordering::Greater) => return non_eq,
    }
    // Tier 6: repo_relative_path ASC.
    match a
        .candidate_ref
        .repo_relative_path
        .as_ref()
        .cmp(b.candidate_ref.repo_relative_path.as_ref())
    {
        Ordering::Equal => {}
        non_eq @ (Ordering::Less | Ordering::Greater) => return non_eq,
    }
    // Tier 7: start_line ASC.
    match a.candidate_ref.start_line.cmp(&b.candidate_ref.start_line) {
        Ordering::Equal => {}
        non_eq @ (Ordering::Less | Ordering::Greater) => return non_eq,
    }
    // Tier 8: doc_id (candidate_id) ASC.
    a.candidate_ref.doc_id.0.cmp(&b.candidate_ref.doc_id.0)
}

/// Compare `Option<f32>` with `None` ordered after any `Some` (`NULL_LAST`)
/// under descending semantics.
fn cmp_opt_f32_desc_null_last(a: Option<f32>, b: Option<f32>) -> Ordering {
    match (a, b) {
        (Some(av), Some(bv)) => bv.total_cmp(&av), // DESC: invert
        (Some(_), None) => Ordering::Less,         // Some sorts before None
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::order_by_merge_tuple;
    use crate::contribution::HybridContribution;
    use crate::types::{CandidateRef, DocId, ManifestGeneration, RepoId};

    fn row(
        doc: u64,
        repo: u64,
        gen_: u64,
        path: &str,
        line: u32,
        fused: f32,
        lex: Option<f32>,
        sem: Option<f32>,
    ) -> HybridContribution {
        HybridContribution {
            candidate_ref: CandidateRef {
                doc_id: DocId(doc),
                repo_id: RepoId(repo),
                generation: ManifestGeneration(gen_),
                repo_relative_path: Box::<str>::from(path),
                start_line: line,
            },
            lex_rank: lex.map(|_| 1),
            lex_score: lex,
            sem_rank: sem.map(|_| 1),
            sem_score: sem,
            fused_score: fused,
        }
    }

    fn ids(v: &[HybridContribution]) -> Vec<u64> {
        v.iter().map(|r| r.candidate_ref.doc_id.0).collect()
    }

    #[test]
    fn fused_score_desc_dominates() {
        let v = vec![
            row(1, 9, 9, "z", 9, 0.10, Some(0.0), Some(0.0)),
            row(2, 1, 1, "a", 1, 0.90, Some(0.0), Some(0.0)),
            row(3, 5, 5, "m", 5, 0.50, Some(0.0), Some(0.0)),
        ];
        assert_eq!(ids(&order_by_merge_tuple(v)), vec![2, 3, 1]);
    }

    #[test]
    fn lex_score_desc_breaks_fused_tie() {
        // Same fused; doc 2 has lex 0.9 > doc 1 lex 0.5; doc 3 has None.
        // Expected order: 2 (0.9), 1 (0.5), 3 (None)
        let v = vec![
            row(1, 1, 1, "a", 1, 0.50, Some(0.5), Some(0.5)),
            row(2, 1, 1, "a", 1, 0.50, Some(0.9), Some(0.5)),
            row(3, 1, 1, "a", 1, 0.50, None, Some(0.5)),
        ];
        assert_eq!(ids(&order_by_merge_tuple(v)), vec![2, 1, 3]);
    }

    #[test]
    fn sem_score_desc_breaks_when_fused_and_lex_tied() {
        let v = vec![
            row(1, 1, 1, "a", 1, 0.50, Some(0.5), Some(0.5)),
            row(2, 1, 1, "a", 1, 0.50, Some(0.5), Some(0.9)),
            row(3, 1, 1, "a", 1, 0.50, Some(0.5), None),
        ];
        assert_eq!(ids(&order_by_merge_tuple(v)), vec![2, 1, 3]);
    }

    #[test]
    fn repo_id_breaks_when_scores_tied() {
        let v = vec![
            row(1, 9, 1, "a", 1, 0.5, Some(0.5), Some(0.5)),
            row(2, 3, 1, "a", 1, 0.5, Some(0.5), Some(0.5)),
            row(3, 7, 1, "a", 1, 0.5, Some(0.5), Some(0.5)),
        ];
        assert_eq!(ids(&order_by_merge_tuple(v)), vec![2, 3, 1]);
    }

    #[test]
    fn generation_breaks_when_scores_and_repo_tied() {
        let v = vec![
            row(1, 4, 5, "a", 1, 0.5, Some(0.5), Some(0.5)),
            row(2, 4, 2, "a", 1, 0.5, Some(0.5), Some(0.5)),
            row(3, 4, 7, "a", 1, 0.5, Some(0.5), Some(0.5)),
        ];
        assert_eq!(ids(&order_by_merge_tuple(v)), vec![2, 1, 3]);
    }

    #[test]
    fn path_breaks_when_scores_repo_generation_tied() {
        let v = vec![
            row(1, 4, 2, "src/z.rs", 1, 0.5, Some(0.5), Some(0.5)),
            row(2, 4, 2, "src/a.rs", 1, 0.5, Some(0.5), Some(0.5)),
            row(3, 4, 2, "src/m.rs", 1, 0.5, Some(0.5), Some(0.5)),
        ];
        assert_eq!(ids(&order_by_merge_tuple(v)), vec![2, 3, 1]);
    }

    #[test]
    fn start_line_breaks_when_path_tied() {
        let v = vec![
            row(1, 4, 2, "a", 99, 0.5, Some(0.5), Some(0.5)),
            row(2, 4, 2, "a", 2, 0.5, Some(0.5), Some(0.5)),
            row(3, 4, 2, "a", 33, 0.5, Some(0.5), Some(0.5)),
        ];
        assert_eq!(ids(&order_by_merge_tuple(v)), vec![2, 3, 1]);
    }

    #[test]
    fn doc_id_breaks_final_tier() {
        let v = vec![
            row(7, 4, 2, "a", 1, 0.5, Some(0.5), Some(0.5)),
            row(2, 4, 2, "a", 1, 0.5, Some(0.5), Some(0.5)),
            row(5, 4, 2, "a", 1, 0.5, Some(0.5), Some(0.5)),
        ];
        assert_eq!(ids(&order_by_merge_tuple(v)), vec![2, 5, 7]);
    }

    #[test]
    fn worked_example_top_5_pinned() {
        // SEM-02 §4.6 worked example. RRF unweighted produces:
        //   A: ~0.01626 (doc 1)
        //   D: ~0.01639 (doc 4)        ← actually 1/61 > 1/61+1/62? no, 1/61 < 1/61+1/62
        // Let's just use canonical scores per the spec.
        // doc A (1): 0.01626  fused (lex=12.5 sem=0.88)
        // doc D (4): 0.00820  (sem=0.92 only)   wait this is wrong too — use raw values
        //
        // We pin the merge-tuple output for fused_scores given:
        let v = vec![
            row(3, 1, 1, "p", 1, 0.00794, Some(9.8), None),
            row(5, 1, 1, "p", 1, 0.00794, None, Some(0.85)),
            row(1, 1, 1, "p", 1, 0.01626, Some(12.5), Some(0.88)),
            row(2, 1, 1, "p", 1, 0.00806, Some(11.0), None),
            row(4, 1, 1, "p", 1, 0.00820, None, Some(0.92)),
        ];
        let out = order_by_merge_tuple(v);
        // Expected: A(1) > D(4) > B(2) > C(3) > E(5)
        // C(3) has lex_score=9.8 (Some), E(5) has lex_score=None → C wins lex tier.
        assert_eq!(ids(&out), vec![1, 4, 2, 3, 5]);
    }

    #[test]
    fn sort_is_stable_for_inputs_already_total() {
        let v = vec![
            row(1, 1, 1, "a", 1, 0.5, Some(0.5), Some(0.5)),
            row(2, 1, 1, "a", 2, 0.5, Some(0.5), Some(0.5)),
            row(3, 1, 1, "a", 3, 0.5, Some(0.5), Some(0.5)),
        ];
        assert_eq!(ids(&order_by_merge_tuple(v)), vec![1, 2, 3]);
    }
}
