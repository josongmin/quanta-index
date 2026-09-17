//! Boot inventory: what was found on disk, what was set aside, and what was
//! proven (QI-BB-026).
//!
//! Boot used to deep-validate every sealed generation on disk, twice for
//! lexical, and stop the daemon on the first defect anywhere. Now each track
//! is inventoried once (identities only), every inventoried generation is
//! seeded as sealed in the readiness ledger, everything the inventory could
//! not trust is quarantined with a path and a reason, and only the active
//! `(lexical, semantic)` pairs are proven physically — once, by the
//! lifecycle's restart rehydrate. Every other generation is proven at its
//! own door when something serves, activates, or builds on it.

use std::sync::{Arc, RwLock};

use anyhow::Result;
use quanta_index_contract::{ManifestGeneration, SearchPlaneTrackKind};
use quanta_index_core::{
    CoreError, MetricPointV1, MetricSourcePort, QuarantinedGenerationV1, RepoMapOpenReportV1,
    SealedGenerationScanPort, count_from_usize,
};
use quanta_index_search_plane::{Ledger, LegacyAuxiliaryMigrationReceipt};

/// One track's inventory outcome.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackInventoryReportV1 {
    /// Generations whose sealed identity was readable and owns its directory;
    /// they were seeded as sealed. Their content is unproven until a door
    /// proves it.
    pub sealed_generations: usize,
    /// Directories the inventory set aside. They are absent from readiness:
    /// a query pinned to one answers `NOT_READY`, an activation or delta on
    /// one is refused, and no door will open it until it is repaired or
    /// removed.
    pub quarantined: Vec<QuarantinedGenerationV1>,
}

/// What boot found and proved.
///
/// Exposed on the assembled runtime so the composition root and the harness
/// can see it. Typed control and CLI surfaces over the quarantine set are a
/// later step; this report is the receipt until then.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BootInventoryReportV1 {
    pub lexical: TrackInventoryReportV1,
    pub semantic: TrackInventoryReportV1,
    /// Active `(lexical, semantic)` pairs the lifecycle proved physically at
    /// boot; each was proven exactly once.
    pub active_pairs_validated: usize,
    /// The one-shot move of pre-catalog auxiliary snapshot files into the
    /// catalog, if this boot performed it (QI-BB-020).
    pub auxiliary_migration: Option<LegacyAuxiliaryMigrationReceipt>,
    /// Auxiliary authority rows restored from the catalog at boot.
    pub auxiliary_rows_restored: u64,
    /// What the `RepoMap` store found on disk: loaded, migrated, swept and
    /// quarantined files (QI-BB-008).
    pub repo_map: RepoMapOpenReportV1,
}

impl TrackInventoryReportV1 {
    fn metric_points(&self, track: &str) -> [MetricPointV1; 2] {
        [
            MetricPointV1::gauge_count(
                format!("boot_{track}_sealed_generations"),
                count_from_usize(self.sealed_generations),
            ),
            MetricPointV1::gauge_count(
                format!("boot_{track}_quarantined_generations"),
                count_from_usize(self.quarantined.len()),
            ),
        ]
    }
}

/// What boot found, as gauges fixed for the life of the process,
/// `boot_…` (QI-BB-015).
impl MetricSourcePort for BootInventoryReportV1 {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let mut points = Vec::with_capacity(12);
        points.extend(self.lexical.metric_points("lexical"));
        points.extend(self.semantic.metric_points("semantic"));
        points.push(MetricPointV1::gauge_count(
            "boot_active_pairs_validated",
            count_from_usize(self.active_pairs_validated),
        ));
        points.push(MetricPointV1::gauge_count(
            "boot_auxiliary_rows_restored",
            self.auxiliary_rows_restored,
        ));
        points.push(MetricPointV1::gauge_count(
            "boot_repomap_snapshots_loaded",
            self.repo_map.snapshots_loaded,
        ));
        points.push(MetricPointV1::gauge_count(
            "boot_repomap_snapshots_migrated",
            self.repo_map.snapshots_migrated,
        ));
        points.push(MetricPointV1::gauge_count(
            "boot_repomap_activations_loaded",
            self.repo_map.activations_loaded,
        ));
        points.push(MetricPointV1::gauge_count(
            "boot_repomap_stale_temporaries_removed",
            self.repo_map.stale_temporaries_removed,
        ));
        points.push(MetricPointV1::gauge_count(
            "boot_repomap_quarantined_files",
            count_from_usize(self.repo_map.quarantined.len()),
        ));
        points.push(MetricPointV1::gauge_count(
            "boot_repomap_activations_without_snapshot",
            count_from_usize(self.repo_map.activations_without_snapshot.len()),
        ));
        Ok(points)
    }
}

/// Seed one track's readiness from its inventory.
///
/// Every inventoried generation is recorded materialized and sealed under
/// its manifest digest; the highest generation also seeds the track-level
/// legacy readiness. Nothing is validated here: the validator runs later,
/// once, for the active pairs only.
pub(super) fn seed_track_readiness(
    ledger: &Arc<RwLock<Ledger>>,
    track: SearchPlaneTrackKind,
    scanner: &dyn SealedGenerationScanPort,
) -> Result<TrackInventoryReportV1> {
    let inventory = scanner
        .inventory_sealed_generations()
        .map_err(anyhow::Error::from)?;
    let mut guard = ledger.write().map_err(|err| {
        anyhow::anyhow!("ledger poisoned during {track:?} readiness bootstrap: {err}")
    })?;
    let mut highest: Option<(ManifestGeneration, String)> = None;
    for candidate in &inventory.sealed {
        if candidate.track != track {
            return Err(anyhow::anyhow!(
                "{track:?} inventory returned a {:?} generation",
                candidate.track
            ));
        }
        guard.record_track_materialized(
            &candidate.repo_id,
            &candidate.revision_id,
            track,
            candidate.manifest_generation,
            Some(candidate.manifest_digest.as_str()),
        );
        guard.record_track_seal_with_digest(
            &candidate.repo_id,
            &candidate.revision_id,
            track,
            candidate.manifest_generation,
            candidate.manifest_digest.as_str(),
        );
        let take_higher = highest
            .as_ref()
            .is_none_or(|(current, _)| candidate.manifest_generation.get() > current.get());
        if take_higher {
            highest = Some((
                candidate.manifest_generation,
                candidate.manifest_digest.clone(),
            ));
        }
    }
    if let Some((generation, digest)) = highest {
        match track {
            SearchPlaneTrackKind::Lexical => {
                guard.lexical_materialize(generation, None);
                guard.lexical_seal(generation);
            }
            SearchPlaneTrackKind::Semantic => {
                guard.semantic_seal_with_digest(generation, digest.as_str());
            }
            SearchPlaneTrackKind::Structural => {
                return Err(anyhow::anyhow!(
                    "structural is an auxiliary authority, not an inventoried search track"
                ));
            }
        }
    }
    drop(guard);
    Ok(TrackInventoryReportV1 {
        sealed_generations: inventory.sealed.len(),
        quarantined: inventory.quarantined,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};

    use quanta_index_contract::{
        GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
    };
    use quanta_index_core::{
        CoreError, GenerationQuarantineReasonV1, QuarantinedGenerationV1,
        SealedGenerationInventoryV1, SealedGenerationScanPort,
    };
    use quanta_index_search_plane::Ledger;

    use super::seed_track_readiness;

    struct ScriptedInventory(SealedGenerationInventoryV1);

    impl SealedGenerationScanPort for ScriptedInventory {
        fn inventory_sealed_generations(&self) -> Result<SealedGenerationInventoryV1, CoreError> {
            Ok(self.0.clone())
        }
    }

    fn snapshot(track: SearchPlaneTrackKind, generation: u64) -> GenerationSnapshot {
        GenerationSnapshot {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            track,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: format!("digest-{generation}"),
        }
    }

    /// Inventoried generations are seeded as sealed under their digest;
    /// quarantined ones are absent from readiness and reported with their
    /// reason.
    #[test]
    fn inventory_seeds_sealed_generations_and_keeps_quarantined_ones_out() {
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let quarantined = QuarantinedGenerationV1 {
            track: SearchPlaneTrackKind::Lexical,
            path: PathBuf::from("/state/indexes/lexical/generation-v1-x/g9"),
            reason: GenerationQuarantineReasonV1::IdentityUnreadable,
            detail: "decode failed".to_string(),
        };
        let scanner = ScriptedInventory(SealedGenerationInventoryV1 {
            sealed: vec![
                snapshot(SearchPlaneTrackKind::Lexical, 3),
                snapshot(SearchPlaneTrackKind::Lexical, 1),
            ],
            quarantined: vec![quarantined.clone()],
        });
        let report = seed_track_readiness(&ledger, SearchPlaneTrackKind::Lexical, &scanner)
            .expect("seed succeeds despite a quarantined generation");
        assert_eq!(report.sealed_generations, 2);
        assert_eq!(report.quarantined, vec![quarantined]);
        // Track readiness is monotonic: the highest inventoried generation is
        // the sealed head under its own digest, and the quarantined g9 —
        // higher than both — did not become the head.
        let (sealed_head, head_digest, legacy_head) = {
            let guard = ledger.read().expect("ledger");
            (
                guard.track_sealed(
                    &RepoId::new("repo"),
                    &RevisionId::new("rev"),
                    SearchPlaneTrackKind::Lexical,
                ),
                guard
                    .track_manifest_digest(
                        &RepoId::new("repo"),
                        &RevisionId::new("rev"),
                        SearchPlaneTrackKind::Lexical,
                    )
                    .map(str::to_string),
                guard.lexical_sealed(),
            )
        };
        assert_eq!(sealed_head, Some(ManifestGeneration::new(3)));
        assert_eq!(head_digest.as_deref(), Some("digest-3"));
        assert_eq!(legacy_head, Some(ManifestGeneration::new(3)));
    }

    /// An inventory that returns the wrong track is a wiring defect, not a
    /// generation to seed.
    #[test]
    fn a_cross_track_inventory_is_refused() {
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let scanner = ScriptedInventory(SealedGenerationInventoryV1 {
            sealed: vec![snapshot(SearchPlaneTrackKind::Semantic, 1)],
            quarantined: Vec::new(),
        });
        let error = seed_track_readiness(&ledger, SearchPlaneTrackKind::Lexical, &scanner)
            .expect_err("cross-track inventory must fail");
        assert!(error.to_string().contains("Semantic"), "{error}");
    }
}
