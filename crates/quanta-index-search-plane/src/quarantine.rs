//! The quarantine control service (QI-BB-026 follow-up, QI-BB-003).
//!
//! Boot sets aside what it cannot trust and serves without it; this is the
//! operator's live view of that set and the one path that removes an
//! entry. The inventory is read from the adapters each time, never from a
//! boot snapshot, so a discard or a repair is visible on the next listing,
//! and a discard names exactly what a listing reported: the adapter
//! re-inventories and refuses anything it does not quarantine at that
//! moment.
//!
//! Two kinds of entry are listed under each track. The adapter's own —
//! a directory whose identity is unreadable, names another scope, or
//! disagrees with its manifest, or a generation whose content the scrub or
//! a gate's re-proof found corrupt — is discarded by the adapter's
//! quarantine port. The search plane's — an **orphan**, a sealed directory whose
//! exact identity the durable search-corpus authority does not retain
//! (reaped and left behind by a crash before reclaim, or sealed before its
//! authority record was written) — is computed here by comparing the
//! adapter's inventory with the ledger's retained identities, is never
//! seeded as sealed, is refused `UNKNOWN_GENERATION` by every query, and
//! is discarded through the sealed-generation reclaim port after the
//! snapshot registry has released it.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    QuarantineDiscardAck, QuarantineDiscardOutcomeDtoV1, QuarantineInventoryV1, QuarantineTargetV1,
    QuarantinedGenerationEntryV1, QuarantinedRepoMapFileEntryV1, RepoId, RevisionId,
    SearchPlaneTrackKind,
};
use quanta_index_core::domains::generation::GenerationStorageKeyV1;
use quanta_index_core::{
    CoreError, GenerationQuarantineReasonV1, InventoriedSealedGenerationV1,
    QuarantineDiscardOutcomeV1, QuarantinedGenerationDiscardPort, QuarantinedGenerationV1,
    QuarantinedRepoMapFileV1, RepoMapQuarantinePort, SealedGenerationInventoryV1,
    SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort, SealedGenerationScanPort,
};

use crate::{
    Ledger, SealedSearchCorpusAuthorityStateV1, SearchCorpusLifecycleOwner, SnapshotKey,
    SnapshotRegistries, SnapshotRetireOutcome, SnapshotRetirementOwner,
};

/// Wire code for discarding an orphan whose handle a query still holds.
pub const QUARANTINE_TARGET_STILL_REFERENCED_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetStillReferenced;

/// One sealed directory the durable authority does not retain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrphanedSealedGenerationV1 {
    pub entry: InventoriedSealedGenerationV1,
    /// Why the authority does not retain it, for the operator.
    pub detail: String,
}

impl OrphanedSealedGenerationV1 {
    /// The quarantine entry this orphan is listed and discarded as.
    #[must_use]
    pub fn quarantined(&self) -> QuarantinedGenerationV1 {
        QuarantinedGenerationV1 {
            track: self.entry.identity.track,
            path: self.entry.path.clone(),
            reason: GenerationQuarantineReasonV1::Orphaned,
            detail: self.detail.clone(),
        }
    }
}

/// Split one track's sealed inventory into what the durable authority
/// retains (the exact generation under the exact digest) and what it
/// does not: the orphans.
///
/// Read under the ledger guard the caller holds; the ledger's sealed
/// identities are the authority's, restored at boot and pruned on reap.
#[must_use]
pub fn partition_sealed_inventory_v1<'a>(
    ledger: &Ledger,
    sealed: &'a [InventoriedSealedGenerationV1],
) -> (
    Vec<&'a InventoriedSealedGenerationV1>,
    Vec<OrphanedSealedGenerationV1>,
) {
    let mut retained = Vec::new();
    let mut orphaned = Vec::new();
    for entry in sealed {
        let identity = &entry.identity;
        let recorded = ledger.sealed_track_identity_digest(
            &identity.repo_id,
            &identity.revision_id,
            identity.track,
            identity.manifest_generation,
        );
        match recorded {
            Some(digest) if digest == identity.manifest_digest => retained.push(entry),
            Some(digest) => orphaned.push(OrphanedSealedGenerationV1 {
                entry: entry.clone(),
                detail: format!(
                    "sealed generation {} of repo={} revision={} carries digest {} but the durable search-corpus authority retains it under digest {digest}",
                    identity.manifest_generation.get(),
                    identity.repo_id.as_str(),
                    identity.revision_id.as_str(),
                    identity.manifest_digest,
                ),
            }),
            None => orphaned.push(OrphanedSealedGenerationV1 {
                entry: entry.clone(),
                detail: format!(
                    "sealed generation {} of repo={} revision={} digest={} is not retained by the durable search-corpus authority (reaped, or sealed before its record was written); nothing serves it",
                    identity.manifest_generation.get(),
                    identity.repo_id.as_str(),
                    identity.revision_id.as_str(),
                    identity.manifest_digest,
                ),
            }),
        }
    }
    (retained, orphaned)
}

/// The ports one [`QuarantineService`] is composed from.
pub struct QuarantineServiceParts {
    pub lexical_scanner: Arc<dyn SealedGenerationScanPort + Send + Sync>,
    pub semantic_scanner: Arc<dyn SealedGenerationScanPort + Send + Sync>,
    pub lexical_discard: Arc<dyn QuarantinedGenerationDiscardPort + Send + Sync>,
    pub semantic_discard: Arc<dyn QuarantinedGenerationDiscardPort + Send + Sync>,
    pub lexical_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    pub semantic_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    pub repo_map: Arc<dyn RepoMapQuarantinePort + Send + Sync>,
    /// The retained identities orphans are computed against.
    pub ledger: Arc<RwLock<Ledger>>,
    /// The durable authority and pair lock that serialize a sealed record
    /// against removal of a crash orphan of the same generation.
    pub lifecycle: Arc<SearchCorpusLifecycleOwner>,
    /// The registries an orphan is released from before its bytes go.
    pub snapshots: SnapshotRegistries,
}

/// One track's ports, so the two tracks share one code path.
struct TrackPorts {
    scanner: Arc<dyn SealedGenerationScanPort + Send + Sync>,
    discard: Arc<dyn QuarantinedGenerationDiscardPort + Send + Sync>,
    reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
}

/// Live quarantine inventory and discard over the adapters' ports.
pub struct QuarantineService {
    lexical: TrackPorts,
    semantic: TrackPorts,
    repo_map: Arc<dyn RepoMapQuarantinePort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
    lifecycle: Arc<SearchCorpusLifecycleOwner>,
    snapshots: SnapshotRegistries,
}

impl QuarantineService {
    #[must_use]
    pub fn new(parts: QuarantineServiceParts) -> Self {
        let QuarantineServiceParts {
            lexical_scanner,
            semantic_scanner,
            lexical_discard,
            semantic_discard,
            lexical_reclaim,
            semantic_reclaim,
            repo_map,
            ledger,
            lifecycle,
            snapshots,
        } = parts;
        Self {
            lexical: TrackPorts {
                scanner: lexical_scanner,
                discard: lexical_discard,
                reclaim: lexical_reclaim,
            },
            semantic: TrackPorts {
                scanner: semantic_scanner,
                discard: semantic_discard,
                reclaim: semantic_reclaim,
            },
            repo_map,
            ledger,
            lifecycle,
            snapshots,
        }
    }

    fn track(&self, track: SearchPlaneTrackKind) -> Result<&TrackPorts, CoreError> {
        match track {
            SearchPlaneTrackKind::Lexical => Ok(&self.lexical),
            SearchPlaneTrackKind::Semantic => Ok(&self.semantic),
            SearchPlaneTrackKind::Structural => Err(CoreError::InvalidContract(
                "quarantine: the structural track has no generation directories to quarantine"
                    .to_string(),
            )),
        }
    }

    /// One track's inventory this instant, and the orphans in it.
    fn track_inventory(
        &self,
        track: SearchPlaneTrackKind,
    ) -> Result<(SealedGenerationInventoryV1, Vec<OrphanedSealedGenerationV1>), CoreError> {
        let inventory = self.track(track)?.scanner.inventory_sealed_generations()?;
        let orphaned = {
            let guard = self
                .ledger
                .read()
                .map_err(|err| CoreError::Storage(format!("quarantine: ledger poisoned: {err}")))?;
            partition_sealed_inventory_v1(&guard, &inventory.sealed).1
        };
        Ok((inventory, orphaned))
    }

    /// Everything quarantined right now, per authority, as the adapters
    /// report it this instant plus the orphans the authority does not
    /// retain.
    pub fn inventory(&self) -> Result<QuarantineInventoryV1, CoreError> {
        let mut lexical = Vec::new();
        let mut semantic = Vec::new();
        for (track, entries) in [
            (SearchPlaneTrackKind::Lexical, &mut lexical),
            (SearchPlaneTrackKind::Semantic, &mut semantic),
        ] {
            let (inventory, orphaned) = self.track_inventory(track)?;
            entries.extend(track_entries(track, &inventory.quarantined)?);
            let orphan_entries: Vec<QuarantinedGenerationV1> = orphaned
                .iter()
                .map(OrphanedSealedGenerationV1::quarantined)
                .collect();
            entries.extend(track_entries(track, &orphan_entries)?);
        }
        Ok(QuarantineInventoryV1 {
            lexical,
            semantic,
            repo_map: self
                .repo_map
                .quarantined_files()?
                .iter()
                .map(repo_map_entry)
                .collect(),
        })
    }

    /// Discard one entry exactly as a listing reported it.
    pub fn discard(&self, target: &QuarantineTargetV1) -> Result<QuarantineDiscardAck, CoreError> {
        let outcome = match target {
            QuarantineTargetV1::Generation(entry) => {
                let core_entry = generation_entry_from_wire(entry)?;
                if core_entry.reason == GenerationQuarantineReasonV1::Orphaned {
                    self.discard_orphan(&core_entry)?
                } else {
                    self.discard_adapter_quarantine(&core_entry)?
                }
            }
            QuarantineTargetV1::RepoMapFile(entry) => {
                self.repo_map
                    .discard_quarantined_file(&QuarantinedRepoMapFileV1 {
                        file_name: entry.file_name.clone(),
                        reason: entry.reason.clone(),
                    })?
            }
        };
        Ok(QuarantineDiscardAck {
            target: target.clone(),
            outcome: match outcome {
                QuarantineDiscardOutcomeV1::Discarded { bytes } => {
                    QuarantineDiscardOutcomeDtoV1::Discarded { bytes }
                }
                QuarantineDiscardOutcomeV1::Absent => QuarantineDiscardOutcomeDtoV1::Absent,
            },
        })
    }

    fn keys_affected_by_path(
        &self,
        track: SearchPlaneTrackKind,
        path: &Path,
        durable_keys: BTreeSet<SnapshotKey>,
    ) -> Result<BTreeSet<SnapshotKey>, CoreError> {
        let mut keys = match track {
            SearchPlaneTrackKind::Lexical => self.snapshots.lexical.known_keys()?,
            SearchPlaneTrackKind::Semantic => self.snapshots.semantic.known_keys()?,
            SearchPlaneTrackKind::Structural => {
                return Err(CoreError::InvalidContract(
                    "structural quarantine has no snapshot registry".into(),
                ));
            }
        };
        let ledger = self
            .ledger
            .read()
            .map_err(|error| CoreError::Storage(format!("quarantine: ledger poisoned: {error}")))?;
        keys.extend(
            ledger
                .sealed_track_keys(track)
                .map(|(repo, revision, generation)| SnapshotKey::new(repo, revision, generation)),
        );
        keys.extend(durable_keys);
        drop(ledger);
        keys.retain(|key| {
            let family = GenerationStorageKeyV1::for_repo_revision(&key.repo_id, &key.revision_id);
            path.ends_with(family.as_str())
                || path.ends_with(family.generation_dir(Path::new(""), key.generation))
        });
        Ok(keys)
    }

    fn finish_fences(
        &self,
        track: SearchPlaneTrackKind,
        keys: &BTreeSet<SnapshotKey>,
    ) -> Result<(), CoreError> {
        for key in keys {
            self.snapshots.finish_retirement(
                track,
                key,
                SnapshotRetirementOwner::AdapterQuarantine,
            )?;
        }
        Ok(())
    }

    /// A corrupt identity may no longer decode, so map the adapter-owned
    /// physical path to the ledger's retained keys and the registry's live
    /// keys before deleting it. A family entry covers all its generations.
    fn discard_adapter_quarantine(
        &self,
        entry: &QuarantinedGenerationV1,
    ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
        let ports = self.track(entry.track)?;
        let listed_now = ports.scanner.inventory_sealed_generations()?.quarantined;
        if !listed_now
            .iter()
            .any(|listed| listed.path == entry.path && listed.reason == entry.reason)
        {
            if listed_now.iter().any(|listed| listed.path == entry.path) {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
                    message: format!(
                        "quarantine discard: {} is listed under a different reason; list it again before deletion",
                        entry.path.display()
                    ),
                });
            }
            return match std::fs::symlink_metadata(&entry.path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Ok(QuarantineDiscardOutcomeV1::Absent)
                }
                Ok(_) => Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
                    message: format!(
                        "quarantine discard: {} was not quarantined at the service boundary; list it again before deletion",
                        entry.path.display()
                    ),
                }),
                Err(error) => Err(CoreError::Storage(format!(
                    "quarantine discard: inspect {}: {error}",
                    entry.path.display()
                ))),
            };
        }
        let authority = self.lifecycle.authority_store();
        let keys = authority.with_record_keys_during_quarantine(|durable_keys| {
            self.keys_affected_by_path(entry.track, &entry.path, durable_keys)
        })?;
        let pairs: BTreeSet<_> = keys
            .iter()
            .map(|key| (key.repo_id.clone(), key.revision_id.clone()))
            .collect();
        if pairs.len() > 1 {
            return Err(CoreError::Storage(format!(
                "quarantine discard: {} maps to more than one retained pair",
                entry.path.display()
            )));
        }
        let coordinator = self.lifecycle.activation_catalog().lifecycle_coordinator();
        let _pair_guard = pairs
            .iter()
            .next()
            .map(|(repo, revision)| coordinator.lock_pair(repo, revision))
            .transpose()?;
        authority.with_record_keys_during_quarantine(|durable_keys| {
            let keys = self.keys_affected_by_path(entry.track, &entry.path, durable_keys)?;
            self.discard_adapter_quarantine_under_root_lock(
                entry,
                ports,
                pairs.iter().next().map(|(repo, revision)| (repo, revision)),
                keys,
            )
        })
    }

    /// The authority root lock spans this entire decision and adapter delete.
    /// If a record appeared between the preliminary lookup and the pair lock,
    /// retry with its now-known pair instead of deleting without that guard.
    fn discard_adapter_quarantine_under_root_lock(
        &self,
        entry: &QuarantinedGenerationV1,
        ports: &TrackPorts,
        locked_pair: Option<(&RepoId, &RevisionId)>,
        keys: BTreeSet<SnapshotKey>,
    ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
        if keys.iter().any(|key| {
            locked_pair
                .is_none_or(|(repo, revision)| key.repo_id != *repo || key.revision_id != *revision)
        }) {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationConflict,
                message: "quarantine discard: path acquired a durable pair while locking its authority; retry with the current inventory".into(),
            });
        }
        let mut retired = BTreeSet::new();
        let mut holders = 0usize;
        for key in &keys {
            let outcome = self.snapshots.retire(
                entry.track,
                key,
                SnapshotRetirementOwner::AdapterQuarantine,
            )?;
            let _new = retired.insert(key.clone());
            if let SnapshotRetireOutcome::StillReferenced { holders: found } = outcome {
                holders = holders.saturating_add(found);
            }
        }
        if holders > 0 {
            self.finish_fences(entry.track, &retired)?;
            return Err(CoreError::Typed {
                code: QUARANTINE_TARGET_STILL_REFERENCED_CODE,
                message: format!(
                    "quarantine discard: {} still has {holders} reader handle(s) or open proof(s)",
                    entry.path.display()
                ),
            });
        }
        let settled = Cell::new(false);
        let on_absent = || {
            for key in &retired {
                self.snapshots.finish_removed_generation(
                    entry.track,
                    key,
                    SnapshotRetirementOwner::AdapterQuarantine,
                )?;
            }
            settled.set(true);
            Ok(())
        };
        let outcome = ports
            .discard
            .discard_quarantined_generation_with_settlement(entry, &on_absent);
        match outcome {
            Ok(outcome) => {
                if !settled.get() {
                    return Err(CoreError::Storage(
                        "quarantine discard omitted namespace settlement".into(),
                    ));
                }
                Ok(outcome)
            }
            Err(
                error @ CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
                    ..
                },
            ) => {
                if !settled.get() {
                    self.finish_fences(entry.track, &retired)?;
                }
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    /// Remove an orphan's directory: re-derive the orphan set this
    /// instant, release the registry's handle (a query still holding one
    /// defers the discard, typed), then reclaim through the port that
    /// re-proves the sealed identity before deleting.
    ///
    /// A path the adapter's inventory no longer names at all is gone and
    /// answers `Absent`; a path it names as something other than an orphan
    /// now — retained again, or set aside by the adapter itself — is
    /// refused typed, never removed.
    fn discard_orphan(
        &self,
        entry: &QuarantinedGenerationV1,
    ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
        let ports = self.track(entry.track)?;
        let (inventory, orphaned) = self.track_inventory(entry.track)?;
        let Some(orphan) = orphaned
            .iter()
            .find(|orphan| orphan.entry.path == entry.path)
        else {
            let named_now = inventory
                .sealed
                .iter()
                .any(|sealed| sealed.path == entry.path)
                || inventory
                    .quarantined
                    .iter()
                    .any(|quarantined| quarantined.path == entry.path);
            if named_now {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
                    message: format!(
                        "quarantine discard: {} is not an orphan now; it is retained by the authority or set aside by the adapter itself and is not this path's to remove",
                        entry.path.display()
                    ),
                });
            }
            return Ok(QuarantineDiscardOutcomeV1::Absent);
        };
        let identity = &orphan.entry.identity;
        // A seal retry can publish a durable record before it reconciles the
        // in-memory ledger. Serialize with that durable mutation and check the
        // authority, then reclassify the physical entry under the same guard.
        let coordinator = self.lifecycle.activation_catalog().lifecycle_coordinator();
        let _pair_guard = coordinator.lock_pair(&identity.repo_id, &identity.revision_id)?;
        match self
            .lifecycle
            .authority_store()
            .inspect_sealed_search_corpus(
                &identity.repo_id,
                &identity.revision_id,
                identity.manifest_generation,
                &identity.manifest_digest,
            )? {
            SealedSearchCorpusAuthorityStateV1::Absent => {}
            SealedSearchCorpusAuthorityStateV1::Exact => {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
                    message: format!(
                        "quarantine discard: {} is retained by the durable search-corpus authority",
                        entry.path.display()
                    ),
                });
            }
        }
        let (_, current_orphans) = self.track_inventory(entry.track)?;
        if !current_orphans.iter().any(|current| {
            current.entry.path == orphan.entry.path && current.entry.identity == *identity
        }) {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
                message: format!(
                    "quarantine discard: {} changed identity or is no longer an orphan",
                    entry.path.display()
                ),
            });
        }
        let key = SnapshotKey::new(
            &identity.repo_id,
            &identity.revision_id,
            identity.manifest_generation,
        );
        let fence = match entry.track {
            SearchPlaneTrackKind::Lexical => self
                .snapshots
                .lexical
                .retire(&key, SnapshotRetirementOwner::OrphanDiscard)?,
            SearchPlaneTrackKind::Semantic => self
                .snapshots
                .semantic
                .retire(&key, SnapshotRetirementOwner::OrphanDiscard)?,
            SearchPlaneTrackKind::Structural => {
                return Err(CoreError::InvalidContract(
                    "quarantine discard: structural is not a search-corpus track".to_string(),
                ));
            }
        };
        if let SnapshotRetireOutcome::StillReferenced { holders } = fence {
            // Nothing was removed. A later seal retry may retain the same
            // identity, so the refusal must not strand its registry key.
            self.snapshots.finish_retirement(
                entry.track,
                &key,
                SnapshotRetirementOwner::OrphanDiscard,
            )?;
            return Err(CoreError::Typed {
                code:
                    quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetStillReferenced,
                message: format!(
                    "quarantine discard: {} still has {holders} reader handle(s) or open flight(s); retry once they finish",
                    entry.path.display()
                ),
            });
        }
        let settled = Cell::new(false);
        let on_absent = || {
            self.snapshots.finish_removed_generation(
                entry.track,
                &key,
                SnapshotRetirementOwner::OrphanDiscard,
            )?;
            settled.set(true);
            Ok(())
        };
        let outcome = match ports
            .reclaim
            .reclaim_sealed_generation_with_settlement(identity, &on_absent)
        {
            Ok(SealedGenerationReclaimOutcomeV1::Reclaimed { bytes }) => {
                QuarantineDiscardOutcomeV1::Discarded { bytes }
            }
            Ok(SealedGenerationReclaimOutcomeV1::Absent) => QuarantineDiscardOutcomeV1::Absent,
            Err(error) => return Err(error),
        };
        if !settled.get() {
            return Err(CoreError::Storage(
                "quarantine orphan reclaim omitted namespace settlement".into(),
            ));
        }
        Ok(outcome)
    }
}

/// One track's quarantined directories as wire entries.
///
/// An adapter that reports another track's entry under this track is a
/// defect, surfaced rather than relabelled.
fn track_entries(
    track: SearchPlaneTrackKind,
    quarantined: &[QuarantinedGenerationV1],
) -> Result<Vec<QuarantinedGenerationEntryV1>, CoreError> {
    quarantined
        .iter()
        .map(|entry| {
            if entry.track != track {
                return Err(CoreError::Storage(format!(
                    "quarantine inventory: the {track:?} scanner reported {} under track {:?}",
                    entry.path.display(),
                    entry.track
                )));
            }
            let path = entry.path.to_str().ok_or_else(|| {
                CoreError::Storage(format!(
                    "quarantine inventory: path {} is not UTF-8 and cannot be named on the wire",
                    entry.path.display()
                ))
            })?;
            Ok(QuarantinedGenerationEntryV1 {
                track: entry.track,
                path: path.to_string(),
                reason: entry.reason.as_code_str().to_string(),
                detail: entry.detail.clone(),
            })
        })
        .collect()
}

fn repo_map_entry(entry: &QuarantinedRepoMapFileV1) -> QuarantinedRepoMapFileEntryV1 {
    QuarantinedRepoMapFileEntryV1 {
        file_name: entry.file_name.clone(),
        reason: entry.reason.clone(),
    }
}

/// The adapter-side entry a wire entry names; a reason code the domain
/// does not know is refused before any port sees it.
fn generation_entry_from_wire(
    entry: &QuarantinedGenerationEntryV1,
) -> Result<QuarantinedGenerationV1, CoreError> {
    let reason = GenerationQuarantineReasonV1::from_code_str(&entry.reason).ok_or_else(|| {
        CoreError::InvalidContract(format!(
            "quarantine discard: unknown quarantine reason `{}`",
            entry.reason
        ))
    })?;
    Ok(QuarantinedGenerationV1 {
        track: entry.track,
        path: std::path::PathBuf::from(&entry.path),
        reason,
        detail: entry.detail.clone(),
    })
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning tests assert with `assert_eq!` on the doubles' recorded calls; a violated expectation is a test failure, not a propagatable error"
)]
pub(crate) mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, RwLock};

    use quanta_index_contract::{
        GenerationSnapshot, ManifestGeneration, QuarantineDiscardOutcomeDtoV1, QuarantineTargetV1,
        QuarantinedGenerationEntryV1, QuarantinedRepoMapFileEntryV1, RepoId, RevisionId,
        SearchPlaneTrackKind,
    };
    use quanta_index_core::{
        CoreError, GenerationQuarantineReasonV1, GenerationStorageKeyV1,
        InventoriedSealedGenerationV1, QUARANTINE_TARGET_NOT_QUARANTINED_CODE,
        QuarantineDiscardOutcomeV1, QuarantinedGenerationDiscardPort, QuarantinedGenerationV1,
        QuarantinedRepoMapFileV1, RepoMapQuarantinePort, RequestBudgetV1, SealedGenerationBytesV1,
        SealedGenerationInventoryV1, SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort,
        SealedGenerationScanPort,
    };

    use super::{
        QUARANTINE_TARGET_STILL_REFERENCED_CODE, QuarantineService, QuarantineServiceParts,
    };
    use crate::query_dispatcher::tests::support::lexical::StubLexicalSearcher;
    use crate::{
        Ledger, OpenedSnapshot, SearchCorpusLifecycleOwner, SnapshotKey, SnapshotRegistries,
        SnapshotRegistryPolicy, SnapshotRetirementOwner,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A scanner over a fixed inventory: the adapter's own quarantine plus
    /// the sealed directories it found.
    struct ScriptedScanner {
        quarantined: Mutex<Vec<QuarantinedGenerationV1>>,
        sealed: Mutex<Vec<InventoriedSealedGenerationV1>>,
    }

    impl SealedGenerationScanPort for ScriptedScanner {
        fn inventory_sealed_generations(&self) -> Result<SealedGenerationInventoryV1, CoreError> {
            Ok(SealedGenerationInventoryV1 {
                sealed: self
                    .sealed
                    .lock()
                    .map_err(|err| CoreError::Storage(err.to_string()))?
                    .clone(),
                quarantined: self
                    .quarantined
                    .lock()
                    .map_err(|err| CoreError::Storage(err.to_string()))?
                    .clone(),
            })
        }
    }

    /// Discards what it was told is quarantined; records every call.
    pub(crate) struct ScriptedDiscard {
        quarantined: Vec<QuarantinedGenerationV1>,
        pub(crate) discarded: Mutex<Vec<QuarantinedGenerationV1>>,
        post_delete_error: AtomicBool,
    }

    impl QuarantinedGenerationDiscardPort for ScriptedDiscard {
        fn discard_quarantined_generation_with_settlement(
            &self,
            entry: &QuarantinedGenerationV1,
            on_absent: &dyn Fn() -> Result<(), CoreError>,
        ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
            if !self.quarantined.contains(entry) {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
                    message: format!("not quarantined now: {}", entry.path.display()),
                });
            }
            if self.post_delete_error.load(Ordering::SeqCst) {
                std::fs::remove_dir_all(&entry.path).map_err(|error| {
                    CoreError::Storage(format!("scripted quarantine delete: {error}"))
                })?;
                on_absent()?;
                return Err(CoreError::Storage(
                    "scripted parent sync failed after delete".into(),
                ));
            }
            self.discarded
                .lock()
                .map_err(|err| CoreError::Storage(err.to_string()))?
                .push(entry.clone());
            on_absent()?;
            Ok(QuarantineDiscardOutcomeV1::Discarded { bytes: 42 })
        }
    }

    /// Reclaims by removing the identity from the scanner's sealed list
    /// (the "directory" is gone afterwards); records what it reclaimed.
    pub(crate) struct ScriptedReclaim {
        scanner: Arc<ScriptedScanner>,
        pub(crate) reclaimed: Mutex<Vec<GenerationSnapshot>>,
    }

    impl SealedGenerationReclaimPort for ScriptedReclaim {
        fn reclaim_sealed_generation_with_settlement(
            &self,
            retired: &GenerationSnapshot,
            on_absent: &dyn Fn() -> Result<(), CoreError>,
        ) -> Result<SealedGenerationReclaimOutcomeV1, CoreError> {
            let mut sealed = self
                .scanner
                .sealed
                .lock()
                .map_err(|err| CoreError::Storage(err.to_string()))?;
            let before = sealed.len();
            sealed.retain(|entry| entry.identity != *retired);
            let unchanged = sealed.len() == before;
            drop(sealed);
            if unchanged {
                on_absent()?;
                return Ok(SealedGenerationReclaimOutcomeV1::Absent);
            }
            self.reclaimed
                .lock()
                .map_err(|err| CoreError::Storage(err.to_string()))?
                .push(retired.clone());
            on_absent()?;
            Ok(SealedGenerationReclaimOutcomeV1::Reclaimed { bytes: 1_000 })
        }

        fn sealed_generations_for_pair(
            &self,
            repo_id: &RepoId,
            revision_id: &RevisionId,
        ) -> Result<Vec<GenerationSnapshot>, CoreError> {
            Ok(self
                .scanner
                .sealed
                .lock()
                .map_err(|err| CoreError::Storage(err.to_string()))?
                .iter()
                .filter(|entry| {
                    entry.identity.repo_id == *repo_id && entry.identity.revision_id == *revision_id
                })
                .map(|entry| entry.identity.clone())
                .collect())
        }

        fn measure_sealed_generations(
            &self,
            _repo_id: &RepoId,
            _revision_id: &RevisionId,
            generations: &BTreeSet<ManifestGeneration>,
        ) -> Result<SealedGenerationBytesV1, CoreError> {
            Ok(SealedGenerationBytesV1 {
                bytes: 1_000_u64
                    .saturating_mul(u64::try_from(generations.len()).map_or(u64::MAX, |n| n)),
                absent: BTreeSet::new(),
            })
        }

        fn finish_interrupted_reclaims(
            &self,
        ) -> Result<quanta_index_core::FinishedReclaims, CoreError> {
            Ok(quanta_index_core::FinishedReclaims::default())
        }
    }

    struct ScriptedRepoMap(Vec<QuarantinedRepoMapFileV1>);

    impl RepoMapQuarantinePort for ScriptedRepoMap {
        fn quarantined_files(&self) -> Result<Vec<QuarantinedRepoMapFileV1>, CoreError> {
            Ok(self.0.clone())
        }

        fn discard_quarantined_file(
            &self,
            entry: &QuarantinedRepoMapFileV1,
        ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
            match self
                .0
                .iter()
                .find(|listed| listed.file_name == entry.file_name)
            {
                Some(listed) if listed == entry => {
                    Ok(QuarantineDiscardOutcomeV1::Discarded { bytes: 7 })
                }
                Some(listed) => Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
                    message: format!(
                        "recorded as `{}` now, not `{}`",
                        listed.reason, entry.reason
                    ),
                }),
                None => Ok(QuarantineDiscardOutcomeV1::Absent),
            }
        }
    }

    pub(crate) fn quarantined(track: SearchPlaneTrackKind, path: &str) -> QuarantinedGenerationV1 {
        QuarantinedGenerationV1 {
            track,
            path: PathBuf::from(path),
            reason: GenerationQuarantineReasonV1::IdentityUnreadable,
            detail: "identity file does not decode".to_string(),
        }
    }

    fn repo() -> RepoId {
        RepoId::new("repo").expect("static fixture ID satisfies canonical policy")
    }

    fn revision() -> RevisionId {
        RevisionId::new("rev").expect("static fixture ID satisfies canonical policy")
    }

    fn sealed(track: SearchPlaneTrackKind, generation: u64) -> InventoriedSealedGenerationV1 {
        let label = match track {
            SearchPlaneTrackKind::Lexical => "lexical",
            SearchPlaneTrackKind::Semantic => "semantic",
            SearchPlaneTrackKind::Structural => "structural",
        };
        InventoriedSealedGenerationV1 {
            identity: GenerationSnapshot {
                repo_id: repo(),
                revision_id: revision(),
                track,
                manifest_generation: ManifestGeneration::new(generation),
                manifest_digest: format!("digest-{generation}"),
            },
            path: PathBuf::from(format!("/root/{label}/pair/g{generation}")),
        }
    }

    /// The doubles one service is built from; handed back so a test can
    /// read what they recorded.
    pub(crate) struct Doubles {
        pub(crate) service: QuarantineService,
        lexical_scanner: Arc<ScriptedScanner>,
        pub(crate) lexical_discard: Arc<ScriptedDiscard>,
        semantic_discard: Arc<ScriptedDiscard>,
        pub(crate) lexical_reclaim: Arc<ScriptedReclaim>,
        pub(crate) snapshots: SnapshotRegistries,
        state_root: tempfile::TempDir,
    }

    /// A service over scripted ports.
    pub(crate) fn service(
        lexical: Vec<QuarantinedGenerationV1>,
        lexical_sealed: Vec<InventoriedSealedGenerationV1>,
        semantic: Vec<QuarantinedGenerationV1>,
        repo_map: Vec<QuarantinedRepoMapFileV1>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Doubles {
        let lexical_scanner = Arc::new(ScriptedScanner {
            quarantined: Mutex::new(lexical.clone()),
            sealed: Mutex::new(lexical_sealed),
        });
        let semantic_scanner = Arc::new(ScriptedScanner {
            quarantined: Mutex::new(semantic.clone()),
            sealed: Mutex::new(Vec::new()),
        });
        let lexical_discard = Arc::new(ScriptedDiscard {
            quarantined: lexical,
            discarded: Mutex::new(Vec::new()),
            post_delete_error: AtomicBool::new(false),
        });
        let semantic_discard = Arc::new(ScriptedDiscard {
            quarantined: semantic,
            discarded: Mutex::new(Vec::new()),
            post_delete_error: AtomicBool::new(false),
        });
        let lexical_reclaim = Arc::new(ScriptedReclaim {
            scanner: Arc::clone(&lexical_scanner),
            reclaimed: Mutex::new(Vec::new()),
        });
        let semantic_reclaim = Arc::new(ScriptedReclaim {
            scanner: Arc::clone(&semantic_scanner),
            reclaimed: Mutex::new(Vec::new()),
        });
        let state_root = tempfile::tempdir().expect("quarantine fixture state root");
        let lifecycle = Arc::new(
            SearchCorpusLifecycleOwner::open(
                state_root.path(),
                crate::readiness::SearchCorpusHistoryRetentionPolicyV1::new(
                    2, 1_048_576, 8, 8_388_608,
                )
                .expect("quarantine fixture retention"),
                Arc::new(crate::readiness::ScriptedIndexBytesV1),
            )
            .expect("quarantine fixture lifecycle"),
        );
        let snapshots = SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT);
        let service = QuarantineService::new(QuarantineServiceParts {
            lexical_scanner: lexical_scanner.clone(),
            semantic_scanner,
            lexical_discard: lexical_discard.clone(),
            semantic_discard: semantic_discard.clone(),
            lexical_reclaim: lexical_reclaim.clone(),
            semantic_reclaim,
            repo_map: Arc::new(ScriptedRepoMap(repo_map)),
            ledger,
            lifecycle,
            snapshots: snapshots.clone(),
        });
        Doubles {
            service,
            lexical_scanner,
            lexical_discard,
            semantic_discard,
            lexical_reclaim,
            snapshots,
            state_root,
        }
    }

    /// The inventory is the adapters' own, per track, with reasons as their
    /// wire codes; a discard routes by track and comes back as the target
    /// named plus the adapter's outcome.
    #[test]
    fn inventory_and_discard_route_by_track_and_carry_the_adapter_outcome() -> TestResult {
        let lexical = quarantined(SearchPlaneTrackKind::Lexical, "/root/lexical/junk");
        let semantic = quarantined(SearchPlaneTrackKind::Semantic, "/root/semantic/g7");
        let doubles = service(
            vec![lexical.clone()],
            Vec::new(),
            vec![semantic],
            vec![QuarantinedRepoMapFileV1 {
                file_name: "stale--marker.json".to_string(),
                reason: "did not decode".to_string(),
            }],
            Arc::new(RwLock::new(Ledger::new())),
        );
        let inventory = doubles.service.inventory()?;
        assert_eq!(inventory.lexical.len(), 1);
        assert_eq!(inventory.semantic.len(), 1);
        assert_eq!(inventory.repo_map.len(), 1);
        let listed = inventory.lexical.first().ok_or("lexical entry")?;
        assert_eq!(
            (listed.track, listed.path.as_str(), listed.reason.as_str()),
            (
                SearchPlaneTrackKind::Lexical,
                "/root/lexical/junk",
                "GENERATION_QUARANTINE_IDENTITY_UNREADABLE"
            )
        );

        let ack = doubles
            .service
            .discard(&QuarantineTargetV1::Generation(listed.clone()))?;
        assert_eq!(
            ack.outcome,
            QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 42 }
        );
        assert_eq!(ack.target, QuarantineTargetV1::Generation(listed.clone()));
        let discarded = doubles
            .lexical_discard
            .discarded
            .lock()
            .map_err(|err| err.to_string())?
            .clone();
        assert_eq!(
            discarded,
            vec![lexical],
            "the lexical port got the lexical entry"
        );

        let ack = doubles.service.discard(&QuarantineTargetV1::RepoMapFile(
            QuarantinedRepoMapFileEntryV1 {
                file_name: "stale--marker.json".to_string(),
                reason: "did not decode".to_string(),
            },
        ))?;
        assert_eq!(
            ack.outcome,
            QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 7 }
        );
        Ok(())
    }

    #[test]
    fn adapter_quarantine_waits_for_a_cached_generation_reader() -> TestResult {
        let key = SnapshotKey::new(&repo(), &revision(), ManifestGeneration::new(7));
        let path =
            quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
                &repo(),
                &revision(),
            )
            .generation_dir(PathBuf::from("/root/lexical").as_path(), key.generation);
        let entry = quarantined(SearchPlaneTrackKind::Lexical, path.to_str().ok_or("path")?);
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        ledger
            .write()
            .map_err(|error| error.to_string())?
            .record_historically_sealed_search_corpus(
                &repo(),
                &revision(),
                key.generation,
                "digest-7",
            );
        let doubles = service(vec![entry], Vec::new(), Vec::new(), Vec::new(), ledger);
        let held = doubles
            .snapshots
            .lexical
            .acquire(&key, &RequestBudgetV1::unbounded(), || {
                Ok(OpenedSnapshot {
                    handle: Arc::new(StubLexicalSearcher::default()),
                    resident_bytes: 1,
                })
            })?
            .handle;
        let listed = doubles
            .service
            .inventory()?
            .lexical
            .into_iter()
            .next()
            .ok_or("quarantine listed")?;
        match doubles
            .service
            .discard(&QuarantineTargetV1::Generation(listed.clone()))
        {
            Err(CoreError::Typed { code, .. })
                if code == QUARANTINE_TARGET_STILL_REFERENCED_CODE => {}
            other => return Err(format!("live reader was not protected: {other:?}").into()),
        }
        assert!(
            doubles
                .lexical_discard
                .discarded
                .lock()
                .map_err(|error| error.to_string())?
                .is_empty()
        );
        drop(held);
        let discarded = doubles
            .service
            .discard(&QuarantineTargetV1::Generation(listed))?;
        assert_eq!(
            discarded.outcome,
            QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 42 }
        );
        assert_eq!(doubles.snapshots.lexical.stats()?.entries, 0);
        Ok(())
    }

    #[test]
    fn canonical_quarantine_without_a_record_or_resident_pair_is_discardable() -> TestResult {
        let generation = ManifestGeneration::new(7);
        let path = GenerationStorageKeyV1::for_repo_revision(&repo(), &revision())
            .generation_dir(Path::new("/root/lexical"), generation);
        let entry = quarantined(SearchPlaneTrackKind::Lexical, path.to_str().ok_or("path")?);
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let doubles = service(
            vec![entry],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Arc::clone(&ledger),
        );
        let listed = doubles.service.inventory()?.lexical.remove(0);
        let ack = doubles
            .service
            .discard(&QuarantineTargetV1::Generation(listed))?;
        assert_eq!(
            ack.outcome,
            QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 42 }
        );
        assert_eq!(
            doubles
                .lexical_discard
                .discarded
                .lock()
                .map_err(|error| error.to_string())?
                .len(),
            1,
        );
        Ok(())
    }

    #[test]
    fn unpublished_pair_cannot_be_discarded_through_an_identity_unreadable_listing() -> TestResult {
        for track in [
            SearchPlaneTrackKind::Lexical,
            SearchPlaneTrackKind::Semantic,
        ] {
            let generation = ManifestGeneration::new(7);
            let track_root = match track {
                SearchPlaneTrackKind::Lexical => "/root/lexical",
                SearchPlaneTrackKind::Semantic => "/root/semantic",
                SearchPlaneTrackKind::Structural => unreachable!("only stored tracks are listed"),
            };
            let path = GenerationStorageKeyV1::for_repo_revision(&repo(), &revision())
                .generation_dir(Path::new(track_root), generation);
            let entry = quarantined(track, path.to_str().ok_or("path")?);
            let doubles = service(
                if track == SearchPlaneTrackKind::Lexical {
                    vec![entry.clone()]
                } else {
                    Vec::new()
                },
                Vec::new(),
                if track == SearchPlaneTrackKind::Semantic {
                    vec![entry]
                } else {
                    Vec::new()
                },
                Vec::new(),
                Arc::new(RwLock::new(Ledger::new())),
            );
            let key = SnapshotKey::new(&repo(), &revision(), generation);
            let publication = doubles.snapshots.begin_publication(&key)?;
            let listed = doubles.service.inventory()?;
            let target = match track {
                SearchPlaneTrackKind::Lexical => listed.lexical.first(),
                SearchPlaneTrackKind::Semantic => listed.semantic.first(),
                SearchPlaneTrackKind::Structural => None,
            }
            .ok_or("quarantine entry")?;
            let result = doubles
                .service
                .discard(&QuarantineTargetV1::Generation(target.clone()));
            assert!(matches!(
                result,
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationConflict,
                    ..
                })
            ));
            let discard = match track {
                SearchPlaneTrackKind::Lexical => &doubles.lexical_discard,
                SearchPlaneTrackKind::Semantic => &doubles.semantic_discard,
                SearchPlaneTrackKind::Structural => unreachable!("only stored tracks are listed"),
            };
            assert!(
                discard
                    .discarded
                    .lock()
                    .map_err(|error| error.to_string())?
                    .is_empty()
            );
            drop(publication);
            let ack = doubles
                .service
                .discard(&QuarantineTargetV1::Generation(target.clone()))?;
            assert_eq!(
                ack.outcome,
                QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 42 }
            );
        }
        Ok(())
    }

    #[test]
    fn a_new_quarantine_listing_cannot_bypass_the_service_fence() -> TestResult {
        let root = tempfile::tempdir()?;
        let path = root.path().join("generation");
        std::fs::create_dir(&path)?;
        let entry = quarantined(
            SearchPlaneTrackKind::Lexical,
            path.to_str().ok_or("generation path")?,
        );
        let doubles = service(
            vec![entry],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Arc::new(RwLock::new(Ledger::new())),
        );
        let listed = doubles.service.inventory()?.lexical.remove(0);
        doubles
            .lexical_scanner
            .quarantined
            .lock()
            .map_err(|error| error.to_string())?
            .clear();
        match doubles
            .service
            .discard(&QuarantineTargetV1::Generation(listed))
        {
            Err(CoreError::Typed { code, .. })
                if code == QUARANTINE_TARGET_NOT_QUARANTINED_CODE => {}
            other => return Err(format!("stale service listing reached adapter: {other:?}").into()),
        }
        assert!(
            doubles
                .lexical_discard
                .discarded
                .lock()
                .map_err(|error| error.to_string())?
                .is_empty()
        );
        assert!(path.is_dir());
        Ok(())
    }

    #[test]
    fn a_delete_followed_by_parent_sync_error_settles_its_fence() -> TestResult {
        let root = tempfile::tempdir()?;
        let key = SnapshotKey::new(&repo(), &revision(), ManifestGeneration::new(7));
        let path =
            quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
                &key.repo_id,
                &key.revision_id,
            )
            .generation_dir(root.path(), key.generation);
        std::fs::create_dir_all(&path)?;
        let entry = quarantined(SearchPlaneTrackKind::Lexical, path.to_str().ok_or("path")?);
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        ledger
            .write()
            .map_err(|error| error.to_string())?
            .record_historically_sealed_search_corpus(
                &repo(),
                &revision(),
                key.generation,
                "digest-7",
            );
        let doubles = service(vec![entry], Vec::new(), Vec::new(), Vec::new(), ledger);
        let _failed_receipt = doubles
            .snapshots
            .lexical
            .retire(&key, SnapshotRetirementOwner::IntegrityScrub)?;
        doubles
            .lexical_discard
            .post_delete_error
            .store(true, Ordering::SeqCst);
        let listed = doubles.service.inventory()?.lexical.remove(0);
        match doubles
            .service
            .discard(&QuarantineTargetV1::Generation(listed))
        {
            Err(CoreError::Storage(message)) if message.contains("parent sync failed") => {}
            other => return Err(format!("expected post-delete sync failure: {other:?}").into()),
        }
        assert!(!path.exists());
        let _replacement = doubles.snapshots.lexical.begin_promotion(&key)?;
        Ok(())
    }

    /// A reason the domain does not know, or an entry the adapter no longer
    /// quarantines, is refused typed before or by the port.
    #[test]
    fn a_stale_or_malformed_target_is_refused_typed() -> TestResult {
        let doubles = service(
            vec![quarantined(
                SearchPlaneTrackKind::Lexical,
                "/root/lexical/junk",
            )],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Arc::new(RwLock::new(Ledger::new())),
        );
        let unknown_reason = QuarantinedGenerationEntryV1 {
            track: SearchPlaneTrackKind::Lexical,
            path: "/root/lexical/junk".to_string(),
            reason: "NOT_A_REASON".to_string(),
            detail: String::new(),
        };
        match doubles
            .service
            .discard(&QuarantineTargetV1::Generation(unknown_reason))
        {
            Err(CoreError::InvalidContract(message)) if message.contains("NOT_A_REASON") => {}
            other => return Err(format!("unknown reason must be refused: {other:?}").into()),
        }
        let repaired_path = doubles.state_root.path().join("repaired");
        std::fs::create_dir(&repaired_path)?;
        let repaired = QuarantinedGenerationEntryV1 {
            track: SearchPlaneTrackKind::Lexical,
            path: repaired_path.to_str().ok_or("repaired path")?.to_string(),
            reason: "GENERATION_QUARANTINE_IDENTITY_UNREADABLE".to_string(),
            detail: String::new(),
        };
        match doubles
            .service
            .discard(&QuarantineTargetV1::Generation(repaired))
        {
            Err(CoreError::Typed { code, .. })
                if code == QUARANTINE_TARGET_NOT_QUARANTINED_CODE => {}
            other => return Err(format!("a stale entry must be refused typed: {other:?}").into()),
        }
        Ok(())
    }

    /// An orphan is listed, reclaimed through the port, then gone.
    ///
    /// A sealed directory the authority does not retain is listed as an
    /// orphan; discarding it goes through the reclaim port, after which
    /// nothing lists it and a second discard is `Absent`. A retained
    /// directory is never listed and never removable through this path.
    #[test]
    fn an_orphan_is_listed_reclaimed_and_then_gone() -> TestResult {
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        ledger
            .write()
            .map_err(|err| err.to_string())?
            .record_historically_sealed_search_corpus(
                &repo(),
                &revision(),
                ManifestGeneration::new(5),
                "digest-5",
            );
        let doubles = service(
            Vec::new(),
            vec![
                sealed(SearchPlaneTrackKind::Lexical, 2),
                sealed(SearchPlaneTrackKind::Lexical, 5),
            ],
            Vec::new(),
            Vec::new(),
            ledger,
        );
        let inventory = doubles.service.inventory()?;
        let [orphan] = inventory.lexical.as_slice() else {
            return Err(format!("expected exactly the orphan listed: {inventory:?}").into());
        };
        assert_eq!(orphan.reason, "GENERATION_QUARANTINE_ORPHANED");
        assert_eq!(orphan.path, "/root/lexical/pair/g2");
        assert!(orphan.detail.contains("generation 2"), "{}", orphan.detail);

        let ack = doubles
            .service
            .discard(&QuarantineTargetV1::Generation(orphan.clone()))?;
        assert_eq!(
            ack.outcome,
            QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 1_000 }
        );
        let reclaimed = doubles
            .lexical_reclaim
            .reclaimed
            .lock()
            .map_err(|err| err.to_string())?
            .clone();
        assert_eq!(
            reclaimed,
            vec![sealed(SearchPlaneTrackKind::Lexical, 2).identity]
        );
        assert!(doubles.service.inventory()?.lexical.is_empty());
        let again = doubles
            .service
            .discard(&QuarantineTargetV1::Generation(orphan.clone()))?;
        assert_eq!(again.outcome, QuarantineDiscardOutcomeDtoV1::Absent);

        // The retained generation cannot be removed by naming it an orphan.
        let forged = QuarantinedGenerationEntryV1 {
            track: SearchPlaneTrackKind::Lexical,
            path: "/root/lexical/pair/g5".to_string(),
            reason: "GENERATION_QUARANTINE_ORPHANED".to_string(),
            detail: String::new(),
        };
        match doubles
            .service
            .discard(&QuarantineTargetV1::Generation(forged))
        {
            Err(CoreError::Typed { code, .. })
                if code == QUARANTINE_TARGET_NOT_QUARANTINED_CODE => {}
            other => {
                return Err(
                    format!("a retained generation must not be discardable: {other:?}").into(),
                );
            }
        }
        Ok(())
    }

    #[test]
    fn a_durable_seal_record_prevents_discard_of_a_ledger_orphan() -> TestResult {
        let doubles = service(
            Vec::new(),
            vec![sealed(SearchPlaneTrackKind::Lexical, 2)],
            Vec::new(),
            Vec::new(),
            Arc::new(RwLock::new(Ledger::new())),
        );
        let orphan = doubles
            .service
            .inventory()?
            .lexical
            .into_iter()
            .next()
            .ok_or("orphan listed before durable publication")?;
        let _receipt = doubles
            .service
            .lifecycle
            .authority_store()
            .record_sealed_search_corpus(
                &repo(),
                &revision(),
                ManifestGeneration::new(2),
                "digest-2",
            )?;
        match doubles
            .service
            .discard(&QuarantineTargetV1::Generation(orphan))
        {
            Err(CoreError::Typed { code, .. })
                if code == QUARANTINE_TARGET_NOT_QUARANTINED_CODE => {}
            other => {
                return Err(format!(
                    "a durable seal without ledger reconciliation was discarded: {other:?}"
                )
                .into());
            }
        }
        assert!(
            doubles
                .lexical_reclaim
                .reclaimed
                .lock()
                .map_err(|error| error.to_string())?
                .is_empty()
        );
        Ok(())
    }

    /// An orphan whose handle a reader still holds is not deleted under
    /// that reader: the discard is refused typed and retried later.
    #[test]
    fn an_orphan_still_held_by_a_reader_is_deferred_typed() -> TestResult {
        let doubles = service(
            Vec::new(),
            vec![sealed(SearchPlaneTrackKind::Lexical, 2)],
            Vec::new(),
            Vec::new(),
            Arc::new(RwLock::new(Ledger::new())),
        );
        let key = SnapshotKey::new(&repo(), &revision(), ManifestGeneration::new(2));
        let held = doubles
            .snapshots
            .lexical
            .acquire(&key, &RequestBudgetV1::unbounded(), || {
                Ok(OpenedSnapshot {
                    handle: Arc::new(StubLexicalSearcher::default()),
                    resident_bytes: 1,
                })
            })?
            .handle;
        let inventory = doubles.service.inventory()?;
        let orphan = inventory.lexical.first().ok_or("orphan listed")?.clone();
        match doubles
            .service
            .discard(&QuarantineTargetV1::Generation(orphan.clone()))
        {
            Err(CoreError::Typed { code, .. })
                if code == QUARANTINE_TARGET_STILL_REFERENCED_CODE => {}
            other => return Err(format!("a held orphan must be deferred typed: {other:?}").into()),
        }
        // A deferred deletion did not remove the namespace, so a later seal
        // may retain it and must not inherit a stuck admission fence.
        let admitted = doubles.snapshots.lexical.begin_promotion(&key)?;
        drop(admitted);
        drop(held);
        let ack = doubles
            .service
            .discard(&QuarantineTargetV1::Generation(orphan))?;
        assert_eq!(
            ack.outcome,
            QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 1_000 }
        );
        Ok(())
    }
}
