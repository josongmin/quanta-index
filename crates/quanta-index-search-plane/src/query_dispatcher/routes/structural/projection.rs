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
) -> Result<Vec<StructuralMatchCandidate>, CoreError> {
    keys.iter()
        .map(|candidate_id| {
            candidates
                .remove(candidate_id)
                .and_then(|bucket| {
                    bucket
                        .into_iter()
                        .min_by(compare_structural_match_candidates)
                })
                .ok_or_else(|| {
                    CoreError::Storage(format!(
                        "structural: selected candidate `{candidate_id}` vanished from the match set"
                    ))
                })
        })
        .collect()
}

pub(super) fn project_structural_query_candidate(
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

/// Preserve the match extent even when the pattern has no capture. Both the
/// public match extent and its capture offsets refer to the authenticated file.
pub(super) fn project_native_structural_hit(
    mut candidate: StructuralMatchCandidate,
    state: &crate::readiness::StructuralAuthorityState,
    pin: &quanta_index_contract::GenerationPin,
) -> Result<quanta_index_contract::native_protocol::NativeDslStructuralHitV1, CoreError> {
    use quanta_index_contract::native_protocol::{NativeDslMatchRangeV1, NativeDslStructuralHitV1};
    let chunk = state
        .chunks()
        .get(&quanta_index_contract::ChunkId::new(
            candidate.candidate_id.clone(),
        ))
        .ok_or_else(|| {
            CoreError::Storage("Native Structural match lost its Source chunk".into())
        })?;
    let invalid = || CoreError::Storage("Native Structural span exceeds its Source chunk".into());
    let absolute = |offset: u32| {
        chunk
            .start_byte
            .checked_add(offset)
            .filter(|value| *value <= chunk.end_byte)
            .ok_or_else(invalid)
    };
    let line_at = |offset: u32| {
        let prefix = chunk
            .text
            .as_bytes()
            .get(..offset as usize)
            .ok_or_else(invalid)?;
        let breaks = u32::try_from(prefix.iter().filter(|&&byte| byte == b'\n').count())
            .map_err(|_| invalid())?;
        chunk.start_line.checked_add(breaks).ok_or_else(invalid)
    };
    let match_range = NativeDslMatchRangeV1 {
        byte_start: absolute(candidate.pattern_start_byte)?,
        byte_end: absolute(candidate.pattern_end_byte)?,
        line_start: line_at(candidate.pattern_start_byte)?,
        line_end: line_at(candidate.pattern_end_byte.saturating_sub(1))?,
    };
    for binding in &mut candidate.bindings {
        binding.start_byte = absolute(binding.start_byte)?;
        binding.end_byte = absolute(binding.end_byte)?;
    }
    Ok(NativeDslStructuralHitV1 {
        candidate: project_structural_query_candidate(candidate),
        match_range,
        source_repo_id: chunk.searchable_repo_id(&pin.repo_id).clone(),
        path: chunk.repo_relative_path.clone(),
        pattern_kind: chunk
            .structural
            .as_ref()
            .map(|data| data.structural_pattern_kind.to_string()),
        matched_node: chunk
            .structural
            .as_ref()
            .map(|data| data.structural_matched_node.to_string()),
    })
}
