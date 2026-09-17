//! Projection of the selected structural match buckets into contract
//! candidates.

use quanta_index_core::{CoreError, StructuralMatchBinding, StructuralMatchCandidate};

use crate::query_dispatcher::routes::structural::buckets::{
    StructuralCandidateBuckets, compare_structural_match_candidates,
};

/// Project the buckets of `keys` — the page, in order — into rows,
/// taking each bucket out of the match set; one row per candidate, its
/// canonical (smallest) match.
///
/// A key without a bucket cannot come from the page selection over this
/// match set and is refused rather than skipped.
pub(super) fn project_structural_page(
    candidates: &mut StructuralCandidateBuckets,
    keys: &[String],
) -> Result<Vec<quanta_index_contract::StructuralCandidate>, CoreError> {
    keys.iter()
        .map(|candidate_id| {
            candidates
                .remove(candidate_id)
                .and_then(|bucket| {
                    bucket
                        .into_iter()
                        .min_by(compare_structural_match_candidates)
                })
                .map(project_structural_query_candidate)
                .ok_or_else(|| {
                    CoreError::Storage(format!(
                        "structural: selected candidate `{candidate_id}` vanished from the match set"
                    ))
                })
        })
        .collect()
}

fn project_structural_query_candidate(
    candidate: StructuralMatchCandidate,
) -> quanta_index_contract::StructuralCandidate {
    quanta_index_contract::StructuralCandidate {
        candidate_id: candidate.candidate_id,
        bindings: candidate
            .bindings
            .into_iter()
            .map(project_structural_query_binding)
            .collect(),
    }
}

fn project_structural_query_binding(
    binding: StructuralMatchBinding,
) -> quanta_index_contract::StructuralBinding {
    quanta_index_contract::StructuralBinding {
        metavariable: binding.metavariable,
        start_byte: binding.start_byte,
        end_byte: binding.end_byte,
        start_line: binding.start_line,
        end_line: binding.end_line,
    }
}
