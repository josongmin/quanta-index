use std::collections::BTreeMap;

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
            .map(|(index, value)| build_entry_v1(bundle, index, value))
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

fn build_entry_v1(bundle: &RepoMapSourceBundleV1, index: usize, value: &str) -> RepoMapEntryV1 {
    let (subject_doc_type, subject_kind, owner_path) = if let Some((path, _subject)) = value.rsplit_once("::") {
        (
            "Symbol".to_string(),
            "bootstrap_symbol".to_string(),
            path.to_string(),
        )
    } else {
        (
            "File".to_string(),
            "bootstrap_file".to_string(),
            value.to_string(),
        )
    };
    let ordinal = u32::try_from(index + 1).unwrap_or(u32::MAX);
    let final_score_millis = 1_000_u32.saturating_sub(ordinal.saturating_sub(1).saturating_mul(73));
    let score = final_score_millis as f32 / 1_000.0;
    RepoMapEntryV1 {
        subject_identity: value.to_string(),
        subject_doc_type,
        subject_kind,
        owner_path,
        score,
        final_score_millis,
        included: true,
        rank: ordinal,
        importance_score_millis: 900_u32.saturating_sub(ordinal.saturating_sub(1).saturating_mul(17)),
        utility_score_millis: 700_u32.saturating_sub(ordinal.saturating_sub(1).saturating_mul(11)),
        freshness_score_millis: 600_u32.saturating_sub(ordinal.saturating_sub(1).saturating_mul(7)),
        evidence_priority_millis: 500_u32.saturating_sub(ordinal.saturating_sub(1).saturating_mul(5)),
        token_budget_hint: 64,
        contributing_signals: BTreeMap::new(),
        projection_evidence_kind: "ParserItemIndex".to_string(),
        projection_authority_artifact_id: format!(
            "repo-map:{}:{}",
            bundle.snapshot_id,
            ordinal,
        ),
        projection_authority_digest: bundle.authority_digest.clone(),
        projection_status: "Complete".to_string(),
        redaction_state: "Unredacted".to_string(),
    }
}
