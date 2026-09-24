//! Boot inventory: what was found on disk, what was set aside, and what was
//! proven (QI-BB-026, QI-BB-003).
//!
//! Boot used to deep-validate every sealed generation on disk, twice for
//! lexical, and stop the daemon on the first defect anywhere. Now each track
//! is inventoried once (identities only), every inventoried generation the
//! durable search-corpus authority retains is seeded as sealed in the
//! readiness ledger, everything the inventory could not trust is
//! quarantined with a path and a reason, every sealed directory the
//! authority does *not* retain is an orphan — reported here, listed by the
//! quarantine inventory, never seeded, never served — and only the active
//! `(lexical, semantic)` pairs are proven physically — once, by the
//! lifecycle's restart rehydrate. Every other generation is proven at its
//! own door when something serves, activates, or builds on it.
//!
//! The durable search-corpus history is restored into the ledger before
//! either track is inventoried: the retained identities are what the
//! inventory is measured against.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind};
use quanta_index_core::{
    CoreError, FinishedReclaims, IntegrityScrubPort, MetricPointV1, MetricSourcePort,
    QuarantinedGenerationV1, RepoMapOpenReportV1, SealedGenerationReclaimPort,
    SealedGenerationScanPort, count_from_usize,
};
use quanta_index_ipc::SocketAccessPolicy;
use quanta_index_search_plane::{
    Ledger, OrphanedSealedGenerationV1, partition_sealed_inventory_v1,
};

use crate::app::socket_access::{SocketAccessPolicies, SocketRole};

/// One track's inventory outcome.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackInventoryReportV1 {
    /// Generations whose sealed identity was readable, owns its directory,
    /// and is retained by the durable search-corpus authority; they were
    /// seeded as sealed. Their content is unproven until a door proves it.
    pub sealed_generations: usize,
    /// Directories the inventory set aside. They are absent from readiness:
    /// a query pinned to one is refused typed (`UNKNOWN_GENERATION` when
    /// the durable authority does not retain it, otherwise the door's own
    /// refusal), an activation or delta on one is refused, and no door will
    /// open it until it is repaired or removed.
    pub quarantined: Vec<QuarantinedGenerationV1>,
    /// Sealed directories the durable search-corpus authority does not
    /// retain (reaped and left behind by a crash before reclaim, or sealed
    /// before their record was written). Not seeded: a query pinned to one
    /// answers `UNKNOWN_GENERATION`. Listed by `quarantine list` under
    /// `GENERATION_QUARANTINE_ORPHANED` and removed by `quarantine discard`.
    pub orphaned: Vec<QuarantinedGenerationV1>,
    /// The scrub receipts found beside the sealed generations; `None` when
    /// no adapter scrubs this track.
    pub scrub: Option<TrackScrubInventoryV1>,
    /// Every sealed directory the inventory found, retained or orphaned,
    /// by the pair generation it belongs to: what the other track's
    /// inventory is measured against for half-sealed pairs.
    pub sealed_directories: BTreeMap<SealedGenerationKey, SealedDirectory>,
    /// What finishing the track's interrupted reclaims did before the
    /// inventory (QI-BB-003).
    pub interrupted_reclaims: InterruptedReclaimsAtBoot,
}

/// The reclaims a crash cut short, as boot found them (QI-BB-003).
///
/// Each was a sealed generation already out of the generation namespace
/// and committed to deletion, so boot finishes it before the inventory.
/// A storage failure does not refuse boot: the entries stay in the track's
/// reclaim area, out of every namespace, and the next reclaim pass of any
/// pair retries them; boot says so in a notice and a gauge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InterruptedReclaimsAtBoot {
    /// Finished: how many entries, and their bytes.
    Finished(FinishedReclaims),
    /// The storage failed finishing them; its message.
    Unfinished(String),
}

impl Default for InterruptedReclaimsAtBoot {
    fn default() -> Self {
        Self::Finished(FinishedReclaims::default())
    }
}

impl InterruptedReclaimsAtBoot {
    /// The boot notice this outcome warrants, if any.
    #[must_use]
    pub fn boot_notice(&self, track: SearchPlaneTrackKind) -> Option<String> {
        match self {
            Self::Finished(finished) if finished.entries == 0 => None,
            Self::Finished(finished) => Some(format!(
                "finished {} interrupted {track:?} reclaims ({} bytes) left by a crash",
                finished.entries, finished.bytes
            )),
            Self::Unfinished(message) => Some(format!(
                "could not finish the interrupted {track:?} reclaims: {message}; they stay in the track's reclaim area, out of every namespace, and the next reclaim pass retries them"
            )),
        }
    }
}

/// Finish the interrupted reclaims of one track at boot (QI-BB-003).
///
/// A refusal fails boot closed like any other inventory refusal; only the
/// storage's failure is reported and left for the next reclaim pass.
pub(super) fn finish_interrupted_reclaims(
    reclaim: &dyn SealedGenerationReclaimPort,
) -> Result<InterruptedReclaimsAtBoot> {
    match reclaim.finish_interrupted_reclaims() {
        Ok(finished) => Ok(InterruptedReclaimsAtBoot::Finished(finished)),
        Err(error) => error
            .into_storage_failure()
            .map(InterruptedReclaimsAtBoot::Unfinished)
            .map_err(anyhow::Error::from),
    }
}

/// The pair generation a sealed directory belongs to.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SealedGenerationKey {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
}

/// One sealed directory as the inventory found it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SealedDirectory {
    pub path: PathBuf,
    /// Whether the durable search-corpus authority retains it.
    pub retained: bool,
}

/// A generation sealed on one track only (QI-BB-029 보완 #4).
///
/// A crash between the two tracks' seals, or before the pair's authority
/// record, leaves one track's generation sealed and the other track with
/// none. The authority never recorded the pair, so the sealed half is
/// also an orphan — never served, listed and discardable by the
/// quarantine surface. The boot report names it as the pair defect it is,
/// with its repair: a `ReplaceGeneration` seal batch for the same
/// generation rebuilds the missing half (the ingest route's typed repair),
/// or `quarantine discard` removes the sealed half.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HalfSealedPair {
    pub key: SealedGenerationKey,
    /// The track whose half is sealed.
    pub sealed_track: SearchPlaneTrackKind,
    pub path: PathBuf,
}

impl HalfSealedPair {
    /// The boot-log line that names the half and both repairs.
    #[must_use]
    pub fn boot_notice(&self) -> String {
        format!(
            "half-sealed pair: repo={} revision={} generation={} is sealed on the {:?} track only, at {}; publish a ReplaceGeneration seal batch for generation {} to rebuild the missing half, or discard the sealed half through `quarantine discard` (it is listed as an orphan)",
            self.key.repo_id.as_str(),
            self.key.revision_id.as_str(),
            self.key.generation.get(),
            self.sealed_track,
            self.path.display(),
            self.key.generation.get(),
        )
    }
}

/// The half-sealed pairs two track inventories reveal: an orphaned sealed
/// directory whose generation has no sealed directory on the other track.
///
/// Only orphans are half-sealed here. A retained generation was recorded
/// by the authority only once both halves sealed; if one half has gone
/// since (discarded as quarantined), the gate that picks the pair refuses
/// it typed. An orphan whose other half is sealed too is a whole pair the
/// authority dropped, listed as two orphans.
#[must_use]
pub fn half_sealed_pairs(
    lexical: &TrackInventoryReportV1,
    semantic: &TrackInventoryReportV1,
) -> Vec<HalfSealedPair> {
    let mut pairs = Vec::new();
    for (track, own, other) in [
        (SearchPlaneTrackKind::Lexical, lexical, semantic),
        (SearchPlaneTrackKind::Semantic, semantic, lexical),
    ] {
        for (key, directory) in &own.sealed_directories {
            if !directory.retained && !other.sealed_directories.contains_key(key) {
                pairs.push(HalfSealedPair {
                    key: key.clone(),
                    sealed_track: track,
                    path: directory.path.clone(),
                });
            }
        }
    }
    pairs
}

/// What the scrub receipts beside one track's sealed generations said at
/// boot (QI-BB-017): how many were never scrubbed to completion and the
/// most recent completion across the rest.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TrackScrubInventoryV1 {
    /// Sealed generations with no completed scrub receipt.
    pub never_scrubbed_generations: usize,
    /// Unix seconds of the most recent completed scrub over any sealed
    /// generation of the track; `None` when none ever completed.
    pub last_completed_unix: Option<u64>,
}

/// Read the scrub receipts of `track` through the scrub ports that own
/// it: identities and receipts only, no content (QI-BB-017).
///
/// `None` when no port scrubs the track, so the report can say "not
/// scrubbed" apart from "scrubbed, nothing completed yet". Candidates of
/// another track a port happens to list are not this track's.
pub(super) fn inventory_scrub_receipts(
    track: SearchPlaneTrackKind,
    ports: &[Arc<dyn IntegrityScrubPort + Send + Sync>],
) -> Result<Option<TrackScrubInventoryV1>> {
    if ports.is_empty() {
        return Ok(None);
    }
    let mut inventory = TrackScrubInventoryV1::default();
    for port in ports {
        let candidates = port.scrub_candidates().map_err(anyhow::Error::from)?;
        for candidate in candidates
            .iter()
            .filter(|candidate| candidate.identity.track == track)
        {
            match candidate.last_completed_unix {
                Some(unix) => {
                    inventory.last_completed_unix = Some(
                        inventory
                            .last_completed_unix
                            .map_or(unix, |current| current.max(unix)),
                    );
                }
                None => {
                    inventory.never_scrubbed_generations =
                        inventory.never_scrubbed_generations.saturating_add(1);
                }
            }
        }
    }
    Ok(Some(inventory))
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
    /// Auxiliary authority rows restored from the catalog at boot.
    pub auxiliary_rows_restored: u64,
    /// What the `RepoMap` store found on disk: loaded, swept and
    /// quarantined files (QI-BB-008).
    pub repo_map: RepoMapOpenReportV1,
    /// The effective access policy each socket was bound under
    /// (QI-BB-014): private, or shared with a group and/or listed users.
    pub socket_access: SocketAccessPolicies,
    /// Generations sealed on one track only (QI-BB-029), each also listed
    /// as an orphan of its track.
    pub half_sealed_pairs: Vec<HalfSealedPair>,
    /// Whether the semantic embedder this daemon serves under is a
    /// development/test profile (QI-BB-007).
    ///
    /// That is the hash embedder, which carries no learned semantics.
    /// Reported as `boot_semantic_profile_is_dev` so no dashboard mistakes
    /// it for a learned one.
    pub semantic_profile_is_dev: bool,
}

impl TrackInventoryReportV1 {
    #[must_use]
    pub const fn with_scrub(mut self, scrub: Option<TrackScrubInventoryV1>) -> Self {
        self.scrub = scrub;
        self
    }

    #[must_use]
    pub fn with_interrupted_reclaims(mut self, interrupted: InterruptedReclaimsAtBoot) -> Self {
        self.interrupted_reclaims = interrupted;
        self
    }

    fn metric_points(&self, track: &str) -> Vec<MetricPointV1> {
        let mut points = vec![
            MetricPointV1::gauge_count(
                format!("boot_{track}_sealed_generations"),
                count_from_usize(self.sealed_generations),
            ),
            MetricPointV1::gauge_count(
                format!("boot_{track}_quarantined_generations"),
                count_from_usize(self.quarantined.len()),
            ),
            MetricPointV1::gauge_count(
                format!("boot_{track}_orphaned_generations"),
                count_from_usize(self.orphaned.len()),
            ),
        ];
        let (finished, unfinished) = match &self.interrupted_reclaims {
            InterruptedReclaimsAtBoot::Finished(finished) => (finished.entries, 0),
            InterruptedReclaimsAtBoot::Unfinished(_message) => (0, 1),
        };
        points.push(MetricPointV1::gauge_count(
            format!("boot_{track}_interrupted_reclaims_finished"),
            finished,
        ));
        points.push(MetricPointV1::gauge_count(
            format!("boot_{track}_interrupted_reclaims_unfinished"),
            unfinished,
        ));
        if let Some(scrub) = self.scrub {
            points.push(MetricPointV1::gauge_count(
                format!("boot_{track}_never_scrubbed_generations"),
                count_from_usize(scrub.never_scrubbed_generations),
            ));
            // A completion time is scraped only once one exists; "never" is
            // the never-scrubbed count above, not a zero timestamp.
            if let Some(unix) = scrub.last_completed_unix {
                points.push(MetricPointV1::gauge_count(
                    format!("boot_{track}_scrub_last_completed_unix"),
                    unix,
                ));
            }
        }
        points
    }
}

/// The gauges that describe one socket's access policy: whether it is
/// shared, how many users it lists, and — only when it names one — the gid
/// it is shared with.
fn socket_access_points(role: SocketRole, policy: &SocketAccessPolicy) -> Vec<MetricPointV1> {
    let name = role.as_str();
    let mut points = vec![MetricPointV1::gauge_count(
        format!("boot_socket_{name}_shared"),
        u64::from(policy.admits_others()),
    )];
    let (group, listed) = match policy {
        SocketAccessPolicy::Private => (None, 0),
        SocketAccessPolicy::Shared(access) => (access.group(), access.allowed_uids().len()),
    };
    points.push(MetricPointV1::gauge_count(
        format!("boot_socket_{name}_allowed_uids"),
        count_from_usize(listed),
    ));
    if let Some(gid) = group {
        points.push(MetricPointV1::gauge_count(
            format!("boot_socket_{name}_group_gid"),
            u64::from(gid),
        ));
    }
    points
}

/// What boot found, as gauges fixed for the life of the process,
/// `boot_…` (QI-BB-015).
impl MetricSourcePort for BootInventoryReportV1 {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let mut points = Vec::with_capacity(24);
        points.extend(self.lexical.metric_points("lexical"));
        points.extend(self.semantic.metric_points("semantic"));
        points.push(MetricPointV1::gauge_count(
            "boot_active_pairs_validated",
            count_from_usize(self.active_pairs_validated),
        ));
        points.push(MetricPointV1::gauge_count(
            "boot_half_sealed_pairs",
            count_from_usize(self.half_sealed_pairs.len()),
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
        for role in SocketRole::ALL {
            points.extend(socket_access_points(
                role,
                self.socket_access.for_role(role),
            ));
        }
        points.push(MetricPointV1::gauge_count(
            "boot_semantic_profile_is_dev",
            u64::from(self.semantic_profile_is_dev),
        ));
        Ok(points)
    }
}

/// Seed one track's readiness from its inventory.
///
/// Every inventoried generation the durable search-corpus authority
/// retains — the ledger's sealed identities, restored before this runs —
/// is recorded materialized and sealed under its manifest digest; the
/// highest of them also seeds the track-level legacy readiness. Every
/// other sealed directory is an orphan: reported, never seeded. Nothing is
/// validated here: the validator runs later, once, for the active pairs
/// only.
pub(super) fn seed_track_readiness(
    ledger: &Arc<RwLock<Ledger>>,
    track: SearchPlaneTrackKind,
    scanner: &dyn SealedGenerationScanPort,
) -> Result<TrackInventoryReportV1> {
    let inventory = scanner
        .inventory_sealed_generations()
        .map_err(anyhow::Error::from)?;
    for candidate in &inventory.sealed {
        if candidate.identity.track != track {
            return Err(anyhow::anyhow!(
                "{track:?} inventory returned a {:?} generation",
                candidate.identity.track
            ));
        }
    }
    let mut guard = ledger.write().map_err(|err| {
        anyhow::anyhow!("ledger poisoned during {track:?} readiness bootstrap: {err}")
    })?;
    let (retained, orphaned) = partition_sealed_inventory_v1(&guard, &inventory.sealed);
    let retained: Vec<_> = retained.into_iter().cloned().collect();
    let key_of = |identity: &quanta_index_contract::GenerationSnapshot| SealedGenerationKey {
        repo_id: identity.repo_id.clone(),
        revision_id: identity.revision_id.clone(),
        generation: identity.manifest_generation,
    };
    let sealed_directories: BTreeMap<SealedGenerationKey, SealedDirectory> = retained
        .iter()
        .map(|entry| (entry, true))
        .chain(orphaned.iter().map(|orphan| (&orphan.entry, false)))
        .map(|(entry, retained)| {
            (
                key_of(&entry.identity),
                SealedDirectory {
                    path: entry.path.clone(),
                    retained,
                },
            )
        })
        .collect();
    let mut highest: Option<(ManifestGeneration, String)> = None;
    for candidate in &retained {
        let identity = &candidate.identity;
        guard.record_track_materialized(
            &identity.repo_id,
            &identity.revision_id,
            track,
            identity.manifest_generation,
            Some(identity.manifest_digest.as_str()),
        );
        guard.record_track_seal_with_digest(
            &identity.repo_id,
            &identity.revision_id,
            track,
            identity.manifest_generation,
            identity.manifest_digest.as_str(),
        );
        let take_higher = highest
            .as_ref()
            .is_none_or(|(current, _)| identity.manifest_generation.get() > current.get());
        if take_higher {
            highest = Some((
                identity.manifest_generation,
                identity.manifest_digest.clone(),
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
        sealed_generations: retained.len(),
        quarantined: inventory.quarantined,
        orphaned: orphaned
            .iter()
            .map(OrphanedSealedGenerationV1::quarantined)
            .collect(),
        scrub: None,
        sealed_directories,
        interrupted_reclaims: InterruptedReclaimsAtBoot::default(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};

    use quanta_index_contract::{
        GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
    };
    use quanta_index_core::{
        CoreError, FinishedReclaims, GenerationQuarantineReasonV1, InventoriedSealedGenerationV1,
        MetricValueV1, QuarantinedGenerationV1, SealedGenerationBytesV1,
        SealedGenerationInventoryV1, SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort,
        SealedGenerationScanPort,
    };
    use quanta_index_search_plane::Ledger;

    use super::{
        HalfSealedPair, InterruptedReclaimsAtBoot, SealedGenerationKey, TrackInventoryReportV1,
        finish_interrupted_reclaims, half_sealed_pairs, seed_track_readiness,
    };

    struct ScriptedInventory(SealedGenerationInventoryV1);

    impl SealedGenerationScanPort for ScriptedInventory {
        fn inventory_sealed_generations(&self) -> Result<SealedGenerationInventoryV1, CoreError> {
            Ok(self.0.clone())
        }
    }

    fn snapshot(track: SearchPlaneTrackKind, generation: u64) -> InventoriedSealedGenerationV1 {
        InventoriedSealedGenerationV1 {
            identity: GenerationSnapshot {
                repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                revision_id: RevisionId::new("rev")
                    .expect("static fixture ID satisfies canonical policy"),
                track,
                manifest_generation: ManifestGeneration::new(generation),
                manifest_digest: format!("digest-{generation}"),
            },
            path: PathBuf::from(format!(
                "/state/indexes/lexical/generation-v1-x/g{generation}"
            )),
        }
    }

    /// A ledger whose durable authority retains `generations` of the pair.
    fn ledger_retaining(generations: &[u64]) -> Arc<RwLock<Ledger>> {
        let mut ledger = Ledger::new();
        for generation in generations {
            ledger.record_historically_sealed_search_corpus(
                &RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                &RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(*generation),
                &format!("digest-{generation}"),
            );
        }
        Arc::new(RwLock::new(ledger))
    }

    /// Inventoried generations the authority retains are seeded as sealed
    /// under their digest; quarantined ones are absent from readiness and
    /// reported with their reason.
    #[test]
    fn inventory_seeds_retained_generations_and_keeps_quarantined_ones_out() {
        let ledger = ledger_retaining(&[1, 3]);
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
        assert!(report.orphaned.is_empty());
        // Track readiness is monotonic: the highest inventoried generation is
        // the sealed head under its own digest, and the quarantined g9 —
        // higher than both — did not become the head.
        let (sealed_head, head_digest, legacy_head) = {
            let guard = ledger.read().expect("ledger");
            (
                guard.track_sealed(
                    &RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                    &RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                    SearchPlaneTrackKind::Lexical,
                ),
                guard
                    .track_manifest_digest(
                        &RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                        &RevisionId::new("rev")
                            .expect("static fixture ID satisfies canonical policy"),
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

    fn sealed_on(track: SearchPlaneTrackKind, generation: u64) -> InventoriedSealedGenerationV1 {
        let mut sealed = snapshot(track, generation);
        sealed.path = PathBuf::from(format!("/state/{track:?}/g{generation}"));
        sealed
    }

    /// A half-sealed pair is an orphan whose generation has no sealed
    /// directory on the other track (QI-BB-029 보완 #4).
    ///
    /// Over one ledger retaining g1 and g2: g1 is whole; g2 is retained
    /// though its semantic half is gone, which is the gate's to refuse, not
    /// a half-sealed pair; g3 is sealed on lexical only and g5 on semantic
    /// only, with no record — the two half-sealed pairs; g4 is sealed on
    /// both tracks with no record, a whole pair the authority dropped.
    #[test]
    fn a_half_sealed_pair_is_an_orphan_without_a_sealed_partner() {
        let ledger = ledger_retaining(&[1, 2]);
        let lexical = ScriptedInventory(SealedGenerationInventoryV1 {
            sealed: [1, 2, 3, 4]
                .map(|generation| sealed_on(SearchPlaneTrackKind::Lexical, generation))
                .to_vec(),
            quarantined: Vec::new(),
        });
        let semantic = ScriptedInventory(SealedGenerationInventoryV1 {
            sealed: [1, 4, 5]
                .map(|generation| sealed_on(SearchPlaneTrackKind::Semantic, generation))
                .to_vec(),
            quarantined: Vec::new(),
        });
        let lexical = seed_track_readiness(&ledger, SearchPlaneTrackKind::Lexical, &lexical)
            .expect("the lexical inventory seeds");
        let semantic = seed_track_readiness(&ledger, SearchPlaneTrackKind::Semantic, &semantic)
            .expect("the semantic inventory seeds");
        let half = |track: SearchPlaneTrackKind, generation: u64| HalfSealedPair {
            key: SealedGenerationKey {
                repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                revision_id: RevisionId::new("rev")
                    .expect("static fixture ID satisfies canonical policy"),
                generation: ManifestGeneration::new(generation),
            },
            sealed_track: track,
            path: PathBuf::from(format!("/state/{track:?}/g{generation}")),
        };
        assert_eq!(
            half_sealed_pairs(&lexical, &semantic),
            vec![
                half(SearchPlaneTrackKind::Lexical, 3),
                half(SearchPlaneTrackKind::Semantic, 5),
            ]
        );
        assert_eq!(
            (lexical.orphaned.len(), semantic.orphaned.len()),
            (2, 2),
            "g3 and g4 are lexical orphans, g4 and g5 semantic ones"
        );
        assert_eq!(
            half(SearchPlaneTrackKind::Semantic, 5).boot_notice(),
            "half-sealed pair: repo=repo revision=rev generation=5 is sealed on the Semantic track only, at /state/Semantic/g5; publish a ReplaceGeneration seal batch for generation 5 to rebuild the missing half, or discard the sealed half through `quarantine discard` (it is listed as an orphan)"
        );
    }

    /// A sealed directory the authority does not retain — by generation, or
    /// by digest — is an orphan: reported with its path, never seeded, and
    /// never the head even when it is the highest generation on disk.
    #[test]
    fn a_sealed_directory_the_authority_does_not_retain_is_an_orphan_not_a_seed() {
        let ledger = ledger_retaining(&[4]);
        let mut reaped = snapshot(SearchPlaneTrackKind::Lexical, 2);
        reaped.path = PathBuf::from("/state/indexes/lexical/generation-v1-x/g2");
        let mut newer_orphan = snapshot(SearchPlaneTrackKind::Lexical, 5);
        newer_orphan.path = PathBuf::from("/state/indexes/lexical/generation-v1-x/g5");
        let mut wrong_digest = snapshot(SearchPlaneTrackKind::Lexical, 4);
        wrong_digest.identity.manifest_digest = "digest-4-forged".to_string();
        let scanner = ScriptedInventory(SealedGenerationInventoryV1 {
            sealed: vec![reaped.clone(), newer_orphan.clone(), wrong_digest.clone()],
            quarantined: Vec::new(),
        });
        let report = seed_track_readiness(&ledger, SearchPlaneTrackKind::Lexical, &scanner)
            .expect("orphans never fail boot");
        assert_eq!(report.sealed_generations, 0);
        let orphaned: Vec<(PathBuf, GenerationQuarantineReasonV1)> = report
            .orphaned
            .iter()
            .map(|entry| (entry.path.clone(), entry.reason))
            .collect();
        assert_eq!(
            orphaned,
            vec![
                (reaped.path, GenerationQuarantineReasonV1::Orphaned),
                (newer_orphan.path, GenerationQuarantineReasonV1::Orphaned),
                (wrong_digest.path, GenerationQuarantineReasonV1::Orphaned),
            ]
        );
        let (track_head, legacy_head) = {
            let guard = ledger.read().expect("ledger");
            (
                guard.track_sealed(
                    &RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                    &RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                    SearchPlaneTrackKind::Lexical,
                ),
                guard.lexical_sealed(),
            )
        };
        assert_eq!(track_head, None, "an orphan must not become the track head");
        assert_eq!(legacy_head, None);
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

    /// A reclaim port that only finishes interrupted reclaims, answering
    /// the scripted outcome.
    struct ScriptedFinish(Result<FinishedReclaims, CoreError>);

    impl SealedGenerationReclaimPort for ScriptedFinish {
        fn reclaim_sealed_generation(
            &self,
            _retired: &GenerationSnapshot,
        ) -> Result<SealedGenerationReclaimOutcomeV1, CoreError> {
            Err(CoreError::NotImplemented(
                "boot reclaims nothing".to_string(),
            ))
        }

        fn sealed_generations_for_pair(
            &self,
            _repo_id: &RepoId,
            _revision_id: &RevisionId,
        ) -> Result<Vec<GenerationSnapshot>, CoreError> {
            Err(CoreError::NotImplemented("boot lists no pair".to_string()))
        }

        fn measure_sealed_generations(
            &self,
            _repo_id: &RepoId,
            _revision_id: &RevisionId,
            _generations: &BTreeSet<ManifestGeneration>,
        ) -> Result<SealedGenerationBytesV1, CoreError> {
            Err(CoreError::NotImplemented(
                "boot measures no pair".to_string(),
            ))
        }

        fn finish_interrupted_reclaims(&self) -> Result<FinishedReclaims, CoreError> {
            self.0.clone()
        }
    }

    /// The two boot gauges of one track's interrupted reclaims:
    /// (finished, unfinished).
    fn interrupted_gauges(report: &TrackInventoryReportV1) -> (MetricValueV1, MetricValueV1) {
        let gauge = |name: &str| {
            report
                .metric_points("lexical")
                .into_iter()
                .find(|point| point.name == name)
                .map(|point| point.value)
                .expect("the gauge is always reported")
        };
        (
            gauge("boot_lexical_interrupted_reclaims_finished"),
            gauge("boot_lexical_interrupted_reclaims_unfinished"),
        )
    }

    /// Boot finishes what interrupted reclaims left (QI-BB-003).
    ///
    /// Finished entries are reported with their bytes; nothing to finish
    /// is no notice. The storage failing does not refuse boot — the entries
    /// stay for the next reclaim pass, and the notice and the unfinished
    /// gauge say so. A refusal fails boot with the refusal itself.
    #[test]
    fn boot_finishes_interrupted_reclaims_and_reports_what_it_could_not() {
        let finished = FinishedReclaims {
            entries: 2,
            bytes: 40,
        };
        let outcome =
            finish_interrupted_reclaims(&ScriptedFinish(Ok(finished))).expect("finishing succeeds");
        assert_eq!(outcome, InterruptedReclaimsAtBoot::Finished(finished));
        let notice = outcome
            .boot_notice(SearchPlaneTrackKind::Lexical)
            .expect("finished entries warrant a notice");
        assert!(
            notice.contains("finished 2 interrupted Lexical reclaims (40 bytes)"),
            "{notice}"
        );
        assert_eq!(
            interrupted_gauges(
                &TrackInventoryReportV1::default().with_interrupted_reclaims(outcome)
            ),
            (MetricValueV1::Gauge(2.0), MetricValueV1::Gauge(0.0))
        );
        let nothing = finish_interrupted_reclaims(&ScriptedFinish(Ok(FinishedReclaims::default())))
            .expect("an empty area finishes");
        assert_eq!(nothing.boot_notice(SearchPlaneTrackKind::Lexical), None);

        let unfinished = finish_interrupted_reclaims(&ScriptedFinish(Err(CoreError::Storage(
            "reclaim: remove /state/.reclaim/x.g2: permission denied".to_string(),
        ))))
        .expect("the storage failing does not refuse boot");
        assert_eq!(
            unfinished,
            InterruptedReclaimsAtBoot::Unfinished(
                "reclaim: remove /state/.reclaim/x.g2: permission denied".to_string()
            )
        );
        let notice = unfinished
            .boot_notice(SearchPlaneTrackKind::Lexical)
            .expect("an unfinished area warrants a notice");
        assert!(
            notice.contains("permission denied")
                && notice.contains("the next reclaim pass retries them"),
            "{notice}"
        );
        assert_eq!(
            interrupted_gauges(
                &TrackInventoryReportV1::default().with_interrupted_reclaims(unfinished)
            ),
            (MetricValueV1::Gauge(0.0), MetricValueV1::Gauge(1.0))
        );

        let refused = finish_interrupted_reclaims(&ScriptedFinish(Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityScopeMismatch,
            message: "not a reclaim entry".to_string(),
        })))
        .expect_err("a refusal fails boot");
        assert!(
            matches!(
                refused.downcast_ref::<CoreError>(),
                Some(CoreError::Typed { code, .. })
                    if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityScopeMismatch
            ),
            "{refused:?}"
        );
    }
}
