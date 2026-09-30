//! A sealed generation's lifecycle ports: validation, scrub, quarantine, discard and reclaim.

use crate::generation_dir::{
    generation_tree_bytes, is_writer_lock_entry, sync_generation_directory,
};
use crate::index_store::{
    read_lexical_sealed_identity, sealed_identity_entry_present, validate_lexical_sealed_identity,
};
use crate::inventory::{discard_quarantined_directory, inventory_sealed_generations};
use crate::sealed_generation::{
    DiscardingVisitor, last_completed_scrub, quarantine_seal_failure_at, quarantined_by_scrub,
    quarantined_by_scrub_at, scrub_step, seal_failure_quarantine_reason, walk_sealed_generation,
    walk_sealed_generation_at,
};
use crate::{GenKey, LexicalAdapter};
use quanta_index_contract::{
    GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
};
use quanta_index_core::domains::generation::{
    GenerationStorageKeyV1, IncompleteGenerationDiscardOutcomeV1, IncompleteGenerationDiscardPort,
    QuarantinedGenerationV1, SealedGenerationBytesV1, SealedGenerationInventoryV1,
    SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort, unique_inode_tree_bytes,
};
use quanta_index_core::{
    CoreError, DoorFindingOutcome, DoorFindingQuarantinePort, FinishedReclaims,
    GenerationIdentityValidatePort, IntegrityScrubBudgetV1, IntegrityScrubCandidateV1,
    IntegrityScrubCursorV1, IntegrityScrubPort, IntegrityScrubReportV1, QuarantineDiscardOutcomeV1,
    QuarantinedGenerationDiscardPort, SealedGenerationIdentityProbePort, SealedGenerationScanPort,
    reclaim_directory,
};
use std::collections::BTreeSet;

impl GenerationIdentityValidatePort for LexicalAdapter {
    fn validate_generation_identity(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<(), CoreError> {
        let (generation_dir, observed) =
            self.sealed_generation_dir_for(candidate, "identity validator")?;
        // The very walk a query's open runs, over the same manifest: every
        // decodable file read once, proved and decoded; the index opened
        // from the proved commit; the segment files proved present at their
        // committed length. What is admitted here is what a query can open.
        let _verified = walk_sealed_generation(&generation_dir, &observed, &mut DiscardingVisitor)?;
        sync_generation_directory(&generation_dir)
    }
}

impl SealedGenerationIdentityProbePort for LexicalAdapter {
    fn probe_sealed_generation_identity(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<(), CoreError> {
        let (_dir, _identity) = self.sealed_generation_dir_for(candidate, "readiness probe")?;
        Ok(())
    }

    fn inventory_sealed_generation_identity(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<bool, CoreError> {
        let key = GenKey {
            repo_id: candidate.repo_id.clone(),
            revision_id: candidate.revision_id.clone(),
            generation: candidate.manifest_generation,
        };
        let dir = self.index_path(&key);
        for component in [dir.parent(), Some(dir.as_path())] {
            let component = component.ok_or_else(|| {
                CoreError::InvalidContract("lexical generation has no family directory".into())
            })?;
            match std::fs::symlink_metadata(component) {
                Ok(metadata) if metadata.file_type().is_dir() => {}
                Ok(_) => return Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => {
                    return Err(CoreError::Storage(format!(
                        "lexical: inspect {} for point inventory: {error}",
                        component.display()
                    )));
                }
            }
        }
        let observed = match crate::inventory::inventory_generation_dir(&self.state_root, &dir) {
            Ok(Some(observed)) => observed,
            Ok(None) | Err(_) => return Ok(false),
        };
        if observed != *candidate {
            return Ok(false);
        }
        match quarantined_by_scrub(&dir) {
            Ok(None) => Ok(true),
            Ok(Some(_)) => Ok(false),
            Err(CoreError::Storage(error)) => Err(CoreError::Storage(error)),
            Err(_) => Ok(false),
        }
    }
}

impl LexicalAdapter {
    fn scrub_with_fence(
        &self,
        generation: &GenerationSnapshot,
        cursor: Option<IntegrityScrubCursorV1>,
        budget: IntegrityScrubBudgetV1,
        before_quarantine: &dyn Fn() -> Result<(), CoreError>,
    ) -> Result<IntegrityScrubReportV1, CoreError> {
        let key = GenKey {
            repo_id: generation.repo_id.clone(),
            revision_id: generation.revision_id.clone(),
            generation: generation.manifest_generation,
        };
        let _mutation = self.generation_mutation_guard(&key)?;
        let _lifecycle = self.directory_lifecycle_read_guard()?;
        let (generation_dir, observed) = self.sealed_generation_dir_for(generation, "scrub")?;
        let mut progress = self.scrub_progress.lock().map_err(|error| {
            CoreError::Storage(format!("lexical scrub progress poisoned: {error}"))
        })?;
        let start = progress.start_or_resume(&observed, cursor)?;
        let report = scrub_step(&generation_dir, &observed, start, budget, before_quarantine)?;
        progress.record(&observed, &report.outcome)?;
        drop(progress);
        Ok(report)
    }
}

impl IntegrityScrubPort for LexicalAdapter {
    fn scrub_candidates(&self) -> Result<Vec<IntegrityScrubCandidateV1>, CoreError> {
        inventory_sealed_generations(&self.state_root)?
            .sealed
            .into_iter()
            .map(|sealed| {
                // A malformed completion receipt proves no completed pass.
                // Rescrub this candidate instead of blocking every other
                // generation in the track's candidate list.
                let last_completed_unix = match last_completed_scrub(&sealed.path, &sealed.identity) {
                    Ok(completed) => completed,
                    Err(CoreError::Typed { code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid, .. }) => None,
                    Err(error) => return Err(error),
                };
                Ok(IntegrityScrubCandidateV1 {
                    identity: sealed.identity,
                    last_completed_unix,
                })
            })
            .collect()
    }

    fn scrub(
        &self,
        generation: &GenerationSnapshot,
        cursor: Option<IntegrityScrubCursorV1>,
        budget: IntegrityScrubBudgetV1,
    ) -> Result<IntegrityScrubReportV1, CoreError> {
        self.scrub_with_fence(generation, cursor, budget, &|| Ok(()))
    }

    fn scrub_with_quarantine_fence(
        &self,
        generation: &GenerationSnapshot,
        cursor: Option<IntegrityScrubCursorV1>,
        budget: IntegrityScrubBudgetV1,
        before_quarantine: &dyn Fn() -> Result<(), CoreError>,
    ) -> Result<IntegrityScrubReportV1, CoreError> {
        self.scrub_with_fence(generation, cursor, budget, before_quarantine)
    }
}

impl DoorFindingQuarantinePort for LexicalAdapter {
    /// Walk the generation again, as every door does, under the directory
    /// lifecycle lock; quarantine it only if that walk proves its content
    /// does not match the seal.
    fn quarantine_door_finding(
        &self,
        generation: &GenerationSnapshot,
    ) -> Result<DoorFindingOutcome, CoreError> {
        let _lifecycle = self.directory_lifecycle_guard()?;
        let (generation_dir, observed) =
            self.sealed_generation_dir_for(generation, "door-finding quarantine")?;
        let root = crate::sealed_generation::open_generation_dir_nofollow(&generation_dir)
            .map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: open generation for door-finding quarantine {}: {error}",
                    generation_dir.display()
                ))
            })?;
        let pinned_identity =
            crate::index_store::read_lexical_sealed_identity_at(&generation_dir, &root)?;
        validate_lexical_sealed_identity(&pinned_identity, &observed)?;
        if let Some(quarantined) = quarantined_by_scrub_at(&root, &generation_dir)? {
            return Ok(DoorFindingOutcome::Quarantined { quarantined });
        }
        match walk_sealed_generation_at(
            &root,
            &generation_dir,
            &observed,
            &mut DiscardingVisitor,
            None,
        ) {
            Ok(_verified) => Ok(DoorFindingOutcome::NotReproduced),
            Err(error) => match seal_failure_quarantine_reason(&error) {
                Some(reason) => {
                    let quarantined = quarantine_seal_failure_at(
                        &root,
                        &generation_dir,
                        reason,
                        &format!("a door found {error}"),
                    )?;
                    Ok(DoorFindingOutcome::Quarantined { quarantined })
                }
                None => Err(error),
            },
        }
    }
}

impl SealedGenerationScanPort for LexicalAdapter {
    fn inventory_sealed_generations(&self) -> Result<SealedGenerationInventoryV1, CoreError> {
        inventory_sealed_generations(&self.state_root)
    }
}

impl QuarantinedGenerationDiscardPort for LexicalAdapter {
    fn discard_quarantined_generation_with_settlement(
        &self,
        entry: &QuarantinedGenerationV1,
        on_absent: &dyn Fn() -> Result<(), CoreError>,
    ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
        if entry.track != SearchPlaneTrackKind::Lexical {
            return Err(CoreError::InvalidContract(format!(
                "lexical quarantine discard received {:?} track",
                entry.track
            )));
        }
        let _lifecycle = self.directory_lifecycle_guard()?;
        let outcome = discard_quarantined_directory(
            &self.state_root,
            &inventory_sealed_generations(&self.state_root)?.quarantined,
            entry,
        );
        match std::fs::symlink_metadata(&entry.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => on_absent()?,
            Ok(_) if outcome.is_ok() => {
                return Err(CoreError::Storage(
                    "lexical quarantine discard reported removal but path remains present".into(),
                ));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical quarantine discard failed ({outcome:?}); namespace probe failed: {error}"
                )));
            }
        }
        let outcome = outcome?;
        let mut progress = self.scrub_progress.lock().map_err(|error| {
            CoreError::Storage(format!("lexical scrub progress poisoned: {error}"))
        })?;
        progress.clear_if_invalidated(|paused| {
            let key = GenKey {
                repo_id: paused.repo_id.clone(),
                revision_id: paused.revision_id.clone(),
                generation: paused.manifest_generation,
            };
            self.index_path(&key).starts_with(&entry.path)
        });
        drop(progress);
        Ok(outcome)
    }
}

impl IncompleteGenerationDiscardPort for LexicalAdapter {
    fn discard_incomplete_generation(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<IncompleteGenerationDiscardOutcomeV1, CoreError> {
        if candidate.track != SearchPlaneTrackKind::Lexical {
            return Err(CoreError::InvalidContract(format!(
                "lexical incomplete-generation discard received {:?} track",
                candidate.track
            )));
        }
        let key = GenKey {
            repo_id: candidate.repo_id.clone(),
            revision_id: candidate.revision_id.clone(),
            generation: candidate.manifest_generation,
        };
        let _mutation = self.generation_mutation_guard(&key)?;
        let _lifecycle = self.directory_lifecycle_read_guard()?;
        let generation_dir = self.index_path(&key);
        let mut writers = self
            .writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
        if !generation_dir.exists() {
            let _stale_writer = writers.remove(&key);
            return Ok(IncompleteGenerationDiscardOutcomeV1::Absent);
        }
        if sealed_identity_entry_present(&generation_dir)? {
            let observed = read_lexical_sealed_identity(&generation_dir)?;
            validate_lexical_sealed_identity(&observed, candidate)?;
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationImmutable,
                message: format!(
                    "lexical: refusing to discard sealed generation {}",
                    candidate.manifest_generation.get()
                ),
            });
        }
        let writer = writers.remove(&key);
        let writer_guard = writer
            .as_ref()
            .map(|handle| {
                handle.lock().map_err(|err| {
                    CoreError::Storage(format!("lexical writer poisoned during discard: {err}"))
                })
            })
            .transpose()?;
        std::fs::remove_dir_all(&generation_dir).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: discard incomplete generation {}: {error}",
                generation_dir.display()
            ))
        })?;
        drop(writer_guard);
        drop(writer);
        drop(writers);
        self.invalidate_regex_match_cache_generation(&key)?;
        Ok(IncompleteGenerationDiscardOutcomeV1::Discarded)
    }
}

impl SealedGenerationReclaimPort for LexicalAdapter {
    fn reclaim_sealed_generation_with_settlement(
        &self,
        retired: &GenerationSnapshot,
        on_absent: &dyn Fn() -> Result<(), CoreError>,
    ) -> Result<SealedGenerationReclaimOutcomeV1, CoreError> {
        if retired.track != SearchPlaneTrackKind::Lexical {
            return Err(CoreError::InvalidContract(format!(
                "lexical sealed-generation reclaim received {:?} track",
                retired.track
            )));
        }
        let key = GenKey {
            repo_id: retired.repo_id.clone(),
            revision_id: retired.revision_id.clone(),
            generation: retired.manifest_generation,
        };
        let generation_dir = self.index_path(&key);
        let _lifecycle = self.directory_lifecycle_guard()?;
        match std::fs::symlink_metadata(&generation_dir) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                on_absent()?;
                return Ok(SealedGenerationReclaimOutcomeV1::Absent);
            }
            Ok(_) => {}
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: inspect generation namespace {}: {error}",
                    generation_dir.display()
                )));
            }
        }
        // Only a sealed generation is this port's to remove; an unsealed
        // directory belongs to the incomplete-generation protocol.
        if !sealed_identity_entry_present(&generation_dir)? {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationNotSealed,
                message: format!(
                    "lexical: refusing to reclaim unsealed generation {} as retired history",
                    retired.manifest_generation.get()
                ),
            });
        }
        let observed = read_lexical_sealed_identity(&generation_dir)?;
        validate_lexical_sealed_identity(&observed, retired)?;
        let bytes = generation_tree_bytes(&generation_dir)?;
        // A sealed generation has no live writer, but a stale handle from an
        // earlier attempt must not outlive the directory.
        {
            let mut writers = self
                .writers
                .lock()
                .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
            let _stale_writer = writers.remove(&key);
        }
        self.scrub_progress
            .lock()
            .map_err(|error| {
                CoreError::Storage(format!("lexical scrub progress poisoned: {error}"))
            })?
            .clear_if_invalidated(|paused| paused == retired);
        // Out of the generation namespace by one durable rename, then
        // removed: a crash leaves a reclaim-area entry, never a partial tree
        // that would read as an unsealed build (QI-BB-003).
        let reclaimed = reclaim_directory(
            &self.state_root,
            &generation_dir,
            &GenerationStorageKeyV1::for_repo_revision(&key.repo_id, &key.revision_id)
                .reclaim_entry_name(key.generation),
        );
        match std::fs::symlink_metadata(&generation_dir) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => on_absent()?,
            Ok(_) if reclaimed.is_ok() => {
                return Err(CoreError::Storage(
                    "lexical reclaim reported removal but generation namespace remains present"
                        .into(),
                ));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical reclaim failed ({reclaimed:?}); namespace probe failed: {error}"
                )));
            }
        }
        reclaimed?;
        self.invalidate_regex_match_cache_generation(&key)?;
        Ok(SealedGenerationReclaimOutcomeV1::Reclaimed { bytes })
    }

    fn finish_interrupted_reclaims(&self) -> Result<FinishedReclaims, CoreError> {
        let _lifecycle = self.directory_lifecycle_guard()?;
        quanta_index_core::finish_interrupted_reclaims(&self.state_root)
    }

    fn sealed_generations_for_pair(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<GenerationSnapshot>, CoreError> {
        let pair_dir = self
            .state_root
            .join(GenerationStorageKeyV1::for_repo_revision(repo_id, revision_id).as_str());
        let mut out = Vec::new();
        if !pair_dir.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(&pair_dir).map_err(|error| {
            CoreError::Storage(format!("lexical: list {}: {error}", pair_dir.display()))
        })? {
            let entry = entry.map_err(|error| {
                CoreError::Storage(format!("lexical: read generation entry: {error}"))
            })?;
            let generation_dir = entry.path();
            if !generation_dir.is_dir() || !sealed_identity_entry_present(&generation_dir)? {
                continue;
            }
            let identity = read_lexical_sealed_identity(&generation_dir)?;
            if identity.repo_id != *repo_id
                || identity.revision_id != *revision_id
                || identity.track != SearchPlaneTrackKind::Lexical
                || self.index_path(&GenKey {
                    repo_id: identity.repo_id.clone(),
                    revision_id: identity.revision_id.clone(),
                    generation: identity.manifest_generation,
                }) != generation_dir
            {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityScopeMismatch,
                    message: format!(
                        "lexical: persisted identity does not own physical path {}",
                        generation_dir.display()
                    ),
                });
            }
            out.push(identity);
        }
        out.sort_by_key(|identity| identity.manifest_generation);
        Ok(out)
    }

    /// Bytes the named generation directories occupy together, by unique
    /// inode: a delta's hard-linked base segments count once. Writer lock
    /// entries are not bytes of the generation.
    fn measure_sealed_generations(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        let mut roots = Vec::with_capacity(generations.len());
        let mut absent = BTreeSet::new();
        for generation in generations {
            let generation_dir = self.index_path(&GenKey {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                generation: *generation,
            });
            if generation_dir.is_dir() {
                roots.push(generation_dir);
            } else {
                let _new = absent.insert(*generation);
            }
        }
        let bytes = unique_inode_tree_bytes(&roots, &is_writer_lock_entry).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: measure sealed generations of repo={} revision={}: {err}",
                repo_id.as_str(),
                revision_id.as_str()
            ))
        })?;
        Ok(SealedGenerationBytesV1 { bytes, absent })
    }
}
