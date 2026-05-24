use quanta_index_contract::{RepoMapSnapshotMetaV1, RepoMapSourceBundleV1};

use crate::model::{RepoMapEntryV1, RepoMapSnapshotV1};

pub struct RepoMapMaterializer;

impl RepoMapMaterializer {
    #[must_use]
    pub fn materialize(bundle: &RepoMapSourceBundleV1) -> RepoMapSnapshotV1 {
        let snapshot_meta = RepoMapSnapshotMetaV1 {
            snapshot_id: bundle.snapshot_id.clone(),
            projection_version: bundle.projection_version,
            authority_digest: bundle.authority_digest.clone(),
            item_index_availability: bundle.item_index_availability.clone(),
            graph_coverage_class: bundle.graph_coverage_class.clone(),
            exactness_summary: bundle.exactness_summary.clone(),
        };
        let entries = bundle
            .entry_identities
            .iter()
            .enumerate()
            .map(|(index, value)| RepoMapEntryV1 {
                subject_identity: value.clone(),
                subject_kind: "bootstrap_subject".to_string(),
                owner_path: value.clone(),
                score: 1.0 / f32::from(u16::try_from(index + 1).unwrap_or(u16::MAX)),
                rank: u32::try_from(index + 1).unwrap_or(u32::MAX),
            })
            .collect();
        RepoMapSnapshotV1 {
            repo_id: bundle.repo_id.clone(),
            revision_id: bundle.revision_id.clone(),
            manifest_generation: bundle.manifest_generation,
            snapshot_meta,
            entries,
        }
    }
}
