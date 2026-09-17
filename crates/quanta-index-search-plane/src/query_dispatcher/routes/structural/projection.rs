//! Projection of structural match buckets into contract candidates.

use quanta_index_core::{StructuralMatchBinding, StructuralMatchCandidate};

use crate::query_dispatcher::routes::structural::buckets::{
    StructuralCandidateBuckets, compare_structural_match_candidates,
};

pub(super) fn project_structural_query_results(
    candidates: StructuralCandidateBuckets,
) -> Vec<quanta_index_contract::StructuralCandidate> {
    candidates
        .into_iter()
        .filter_map(|(_, bucket)| {
            bucket
                .into_iter()
                .min_by(compare_structural_match_candidates)
        })
        .map(project_structural_query_candidate)
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
