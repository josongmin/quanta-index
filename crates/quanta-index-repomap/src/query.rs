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
                request
                    .focus_subjects
                    .iter()
                    .any(|focus| entry.subject_identity.contains(focus))
            });
        }
        entries.sort_by(|lhs, rhs| rhs.score.total_cmp(&lhs.score).then(lhs.rank.cmp(&rhs.rank)));
        let capped = request.token_budget / 64;
        let limit = request.top_k.min(capped.max(1));
        entries.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(RepoMapQueryResponseV1 {
            repo_id: snapshot.repo_id.clone(),
            revision_id: snapshot.revision_id.clone(),
            manifest_generation: snapshot.manifest_generation,
            snapshot_meta: snapshot.snapshot_meta.clone(),
            entries: entries.into_iter().map(|entry| entry.to_dto()).collect(),
        })
    }
}
