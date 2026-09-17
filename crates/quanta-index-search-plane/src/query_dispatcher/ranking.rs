//! Deterministic tie-breaking for ranked lexical and semantic candidates.

use quanta_index_contract::LexicalCandidate;
use quanta_index_core::SemanticSearchHitV1;

pub(super) fn stabilize_ranked_candidates(results: &mut [LexicalCandidate]) {
    results.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| {
                left.repo_relative_path
                    .as_str()
                    .cmp(right.repo_relative_path.as_str())
            })
            .then(left.start_line.cmp(&right.start_line))
            .then(left.end_line.cmp(&right.end_line))
            .then_with(|| left.candidate_id.as_str().cmp(right.candidate_id.as_str()))
    });
}

pub(super) fn stabilize_semantic_seed_hits_v1(results: &mut [SemanticSearchHitV1]) {
    results.sort_by(|left, right| {
        right
            .candidate
            .score
            .total_cmp(&left.candidate.score)
            .then_with(|| {
                left.candidate
                    .repo_relative_path
                    .as_str()
                    .cmp(right.candidate.repo_relative_path.as_str())
            })
            .then(left.candidate.start_line.cmp(&right.candidate.start_line))
            .then(left.candidate.end_line.cmp(&right.candidate.end_line))
            .then_with(|| left.owner_id.as_str().cmp(right.owner_id.as_str()))
            .then_with(|| left.record_id.as_str().cmp(right.record_id.as_str()))
            .then_with(|| {
                left.candidate
                    .candidate_id
                    .as_str()
                    .cmp(right.candidate.candidate_id.as_str())
            })
    });
}
