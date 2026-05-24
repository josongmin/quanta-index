use quanta_index_contract::{RepoMapQueryRequestV1, RepoMapQueryResponseV1};
use quanta_index_core::{CoreError, RepoMapPolicy};

use crate::model::RepoMapSnapshotV1;

pub struct RepoMapQueryEngine;

impl RepoMapQueryEngine {
    pub fn query(
        snapshot: &RepoMapSnapshotV1,
        request: &RepoMapQueryRequestV1,
    ) -> Result<RepoMapQueryResponseV1, CoreError> {
        RepoMapPolicy::validate_query(request)?;
        let mut entries = snapshot.entries.clone();
        if !request.focus_subjects.is_empty() {
            entries.retain(|entry| {
                request.focus_subjects.iter().any(|focus| {
                    entry.subject_identity == focus.subject_identity
                        && entry.subject_doc_type == focus.subject_doc_type
                })
            });
        }
        entries.sort_by(|lhs, rhs| {
            rhs.final_score_millis
                .cmp(&lhs.final_score_millis)
                .then(lhs.subject_identity.cmp(&rhs.subject_identity))
        });
        let capped = request.token_budget / 64;
        let limit = request.top_k.min(capped.max(1));
        let include_len = usize::try_from(limit)
            .unwrap_or(usize::MAX)
            .min(entries.len());
        for (index, entry) in entries.iter_mut().enumerate() {
            if index < include_len {
                entry.included = true;
                entry.rank = u32::try_from(index + 1).unwrap_or(u32::MAX);
            } else {
                entry.included = false;
                entry.rank = 0;
            }
        }
        let dropped_entries_count =
            u32::try_from(entries.len().saturating_sub(include_len)).unwrap_or(u32::MAX);
        let drop_reason_codes = if dropped_entries_count == 0 {
            Vec::new()
        } else {
            vec!["token_budget_exhausted".to_string()]
        };
        Ok(RepoMapQueryResponseV1 {
            repo_id: snapshot.repo_id.clone(),
            revision_id: snapshot.revision_id.clone(),
            manifest_generation: snapshot.manifest_generation,
            snapshot_meta: snapshot.snapshot_meta.clone(),
            entries: entries.into_iter().map(|entry| entry.to_dto()).collect(),
            dropped_entries_count,
            drop_reason_codes,
            degraded_reason_codes: vec!["external_query_bootstrap".to_string()],
        })
    }
}
