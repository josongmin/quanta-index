//! Candidate-bucket algebra (union / intersect / subtract) and canonical
//! ordering for structural match candidates.

use std::collections::BTreeMap;

use quanta_index_core::{StructuralMatchBinding, StructuralMatchCandidate};

pub(super) type StructuralCandidateBuckets = BTreeMap<String, Vec<StructuralMatchCandidate>>;

pub(super) fn structural_candidate_scope_ids(
    candidates: &StructuralCandidateBuckets,
) -> Vec<String> {
    candidates.keys().cloned().collect()
}

pub(super) fn bucket_structural_matches(
    candidates: Vec<StructuralMatchCandidate>,
) -> StructuralCandidateBuckets {
    let mut buckets = StructuralCandidateBuckets::new();
    for mut candidate in candidates {
        normalize_structural_match_candidate(&mut candidate);
        buckets
            .entry(candidate.candidate_id.clone())
            .or_default()
            .push(candidate);
    }
    for bucket in buckets.values_mut() {
        normalize_structural_match_bucket(bucket);
    }
    buckets
}

pub(super) fn union_structural_buckets(
    mut left: StructuralCandidateBuckets,
    right: StructuralCandidateBuckets,
) -> StructuralCandidateBuckets {
    for (candidate_id, mut matches) in right {
        left.entry(candidate_id).or_default().append(&mut matches);
    }
    for bucket in left.values_mut() {
        normalize_structural_match_bucket(bucket);
    }
    left
}

pub(super) fn intersect_structural_buckets(
    left: &StructuralCandidateBuckets,
    right: &StructuralCandidateBuckets,
) -> StructuralCandidateBuckets {
    let mut merged = StructuralCandidateBuckets::new();
    for (candidate_id, left_matches) in left {
        let Some(right_matches) = right.get(candidate_id) else {
            continue;
        };
        let combined = if left_matches
            .iter()
            .all(|candidate| candidate.bindings.is_empty())
        {
            vec![merge_structural_identity_matches(
                candidate_id,
                right_matches,
            )]
        } else {
            merge_structural_match_sets(candidate_id, left_matches, right_matches)
        };
        if !combined.is_empty() {
            let _prior = merged.insert(candidate_id.clone(), combined);
        }
    }
    merged
}

pub(super) fn subtract_structural_buckets(
    base: &StructuralCandidateBuckets,
    blocked: &StructuralCandidateBuckets,
) -> StructuralCandidateBuckets {
    let mut remaining = StructuralCandidateBuckets::new();
    for (candidate_id, base_matches) in base {
        let next_matches = blocked.get(candidate_id).map_or_else(
            || base_matches.clone(),
            |blocked_matches| {
                base_matches
                    .iter()
                    .filter(|candidate| {
                        !blocked_matches.iter().any(|blocked_candidate| {
                            structural_match_candidates_consistent(candidate, blocked_candidate)
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            },
        );
        if !next_matches.is_empty() {
            let _prior = remaining.insert(candidate_id.clone(), next_matches);
        }
    }
    remaining
}

fn merge_structural_identity_matches(
    candidate_id: &str,
    matches: &[StructuralMatchCandidate],
) -> StructuralMatchCandidate {
    let mut bindings = Vec::new();
    let mut pattern_start_byte = matches
        .first()
        .map_or(0, |candidate| candidate.pattern_start_byte);
    let mut pattern_end_byte = matches
        .first()
        .map_or(0, |candidate| candidate.pattern_end_byte);
    for candidate in matches {
        pattern_start_byte = pattern_start_byte.min(candidate.pattern_start_byte);
        pattern_end_byte = pattern_end_byte.min(candidate.pattern_end_byte);
        for binding in &candidate.bindings {
            if !bindings.iter().any(|existing| existing == binding) {
                bindings.push(binding.clone());
            }
        }
    }
    bindings.sort_by(compare_structural_bindings);
    StructuralMatchCandidate {
        candidate_id: candidate_id.to_string(),
        pattern_start_byte,
        pattern_end_byte,
        bindings,
    }
}

fn merge_structural_match_sets(
    candidate_id: &str,
    left: &[StructuralMatchCandidate],
    right: &[StructuralMatchCandidate],
) -> Vec<StructuralMatchCandidate> {
    let mut merged = Vec::new();
    for left_candidate in left {
        for right_candidate in right {
            if !structural_match_candidates_consistent(left_candidate, right_candidate) {
                continue;
            }
            let mut bindings = left_candidate.bindings.clone();
            for binding in &right_candidate.bindings {
                if !bindings.iter().any(|existing| existing == binding) {
                    bindings.push(binding.clone());
                }
            }
            bindings.sort_by(compare_structural_bindings);
            let (pattern_start_byte, pattern_end_byte) = std::cmp::min(
                (
                    left_candidate.pattern_start_byte,
                    left_candidate.pattern_end_byte,
                ),
                (
                    right_candidate.pattern_start_byte,
                    right_candidate.pattern_end_byte,
                ),
            );
            merged.push(StructuralMatchCandidate {
                candidate_id: candidate_id.to_string(),
                pattern_start_byte,
                pattern_end_byte,
                bindings,
            });
        }
    }
    normalize_structural_match_bucket(&mut merged);
    merged
}

fn structural_match_candidates_consistent(
    left: &StructuralMatchCandidate,
    right: &StructuralMatchCandidate,
) -> bool {
    left.bindings.iter().all(|left_binding| {
        right.bindings.iter().all(|right_binding| {
            left_binding.metavariable != right_binding.metavariable
                || compare_structural_bindings(left_binding, right_binding).is_eq()
        })
    })
}

pub(super) fn normalize_structural_match_bucket(bucket: &mut Vec<StructuralMatchCandidate>) {
    for candidate in bucket.iter_mut() {
        normalize_structural_match_candidate(candidate);
    }
    bucket.sort_by(compare_structural_match_candidates);
    bucket.dedup_by(|left, right| {
        left.candidate_id == right.candidate_id
            && left.pattern_start_byte == right.pattern_start_byte
            && left.pattern_end_byte == right.pattern_end_byte
            && left.bindings == right.bindings
    });
}

fn normalize_structural_match_candidate(candidate: &mut StructuralMatchCandidate) {
    candidate.bindings.sort_by(compare_structural_bindings);
    candidate.bindings.dedup_by(|left, right| left == right);
}

pub(super) fn compare_structural_match_candidates(
    left: &StructuralMatchCandidate,
    right: &StructuralMatchCandidate,
) -> std::cmp::Ordering {
    left.pattern_start_byte
        .cmp(&right.pattern_start_byte)
        .then_with(|| left.pattern_end_byte.cmp(&right.pattern_end_byte))
        .then_with(|| compare_structural_binding_lists(&left.bindings, &right.bindings))
        .then_with(|| left.candidate_id.as_str().cmp(right.candidate_id.as_str()))
}

fn compare_structural_binding_lists(
    left: &[StructuralMatchBinding],
    right: &[StructuralMatchBinding],
) -> std::cmp::Ordering {
    for (left_binding, right_binding) in left.iter().zip(right.iter()) {
        let ordering = compare_structural_bindings(left_binding, right_binding);
        if !ordering.is_eq() {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

fn compare_structural_bindings(
    left: &StructuralMatchBinding,
    right: &StructuralMatchBinding,
) -> std::cmp::Ordering {
    left.metavariable
        .as_str()
        .cmp(right.metavariable.as_str())
        .then_with(|| left.start_byte.cmp(&right.start_byte))
        .then_with(|| left.end_byte.cmp(&right.end_byte))
        .then_with(|| left.start_line.cmp(&right.start_line))
        .then_with(|| left.end_line.cmp(&right.end_line))
}
