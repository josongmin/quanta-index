//! A sealed generation's lifecycle ports: validation, scrub, quarantine, discard and reclaim.

use crate::generation_dir::{
    generation_tree_bytes, is_writer_lock_entry, sync_generation_directory,
};
use crate::index_store::{
    lexical_sealed_identity_path, read_lexical_sealed_identity, validate_lexical_sealed_identity,
};
use crate::inventory::{discard_quarantined_directory, inventory_sealed_generations};
use crate::sealed_generation::{
    DiscardingVisitor, last_completed_scrub, quarantine_content_corrupt, quarantined_by_scrub,
    scrub_step, walk_sealed_generation,
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
    CoreError, DoorFindingOutcome, DoorFindingQuarantinePort, GENERATION_SIDECAR_CORRUPT_CODE,
    GenerationIdentityValidatePort, IntegrityScrubBudgetV1, IntegrityScrubCandidateV1,
    IntegrityScrubCursorV1, IntegrityScrubPort, IntegrityScrubReportV1, QuarantineDiscardOutcomeV1,
    QuarantinedGenerationDiscardPort, SealedGenerationScanPort,
};
use std::collections::BTreeSet;
use std::fs::File;

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

impl IntegrityScrubPort for LexicalAdapter {
    fn scrub_candidates(&self) -> Result<Vec<IntegrityScrubCandidateV1>, CoreError> {
        inventory_sealed_generations(&self.state_root)?
            .sealed
            .into_iter()
            .map(|sealed| {
                let last_completed_unix = last_completed_scrub(&sealed.path, &sealed.identity)?;
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
        let _lifecycle = self.directory_lifecycle_guard()?;
        let (generation_dir, observed) = self.sealed_generation_dir_for(generation, "scrub")?;
        scrub_step(&generation_dir, &observed, cursor, budget)
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
        if let Some(quarantined) = quarantined_by_scrub(&generation_dir)? {
            return Ok(DoorFindingOutcome::Quarantined { quarantined });
        }
        match walk_sealed_generation(&generation_dir, &observed, &mut DiscardingVisitor) {
            Ok(_verified) => Ok(DoorFindingOutcome::NotReproduced),
            Err(CoreError::Typed { code, message }) if code == GENERATION_SIDECAR_CORRUPT_CODE => {
                let quarantined =
                    quarantine_content_corrupt(&generation_dir, format!("a door found {message}"))?;
                Ok(DoorFindingOutcome::Quarantined { quarantined })
            }
            Err(other) => Err(other),
        }
    }
}

impl SealedGenerationScanPort for LexicalAdapter {
    fn inventory_sealed_generations(&self) -> Result<SealedGenerationInventoryV1, CoreError> {
        inventory_sealed_generations(&self.state_root)
    }
}

impl QuarantinedGenerationDiscardPort for LexicalAdapter {
    fn discard_quarantined_generation(
        &self,
        entry: &QuarantinedGenerationV1,
    ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
        if entry.track != SearchPlaneTrackKind::Lexical {
            return Err(CoreError::InvalidContract(format!(
                "lexical quarantine discard received {:?} track",
                entry.track
            )));
        }
        let _lifecycle = self.directory_lifecycle_guard()?;
        discard_quarantined_directory(
            &self.state_root,
            &inventory_sealed_generations(&self.state_root)?.quarantined,
            entry,
        )
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
        let generation_dir = self.index_path(&key);
        let mut writers = self
            .writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
        if !generation_dir.exists() {
            let _stale_writer = writers.remove(&key);
            return Ok(IncompleteGenerationDiscardOutcomeV1::Absent);
        }
        if lexical_sealed_identity_path(&generation_dir).exists() {
            let observed = read_lexical_sealed_identity(&generation_dir)?;
            validate_lexical_sealed_identity(&observed, candidate)?;
            return Err(CoreError::Typed {
                code: "GENERATION_IMMUTABLE".to_string(),
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
    fn reclaim_sealed_generation(
        &self,
        retired: &GenerationSnapshot,
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
        if !generation_dir.exists() {
            return Ok(SealedGenerationReclaimOutcomeV1::Absent);
        }
        // Only a sealed generation is this port's to remove; an unsealed
        // directory belongs to the incomplete-generation protocol.
        if !lexical_sealed_identity_path(&generation_dir).exists() {
            return Err(CoreError::Typed {
                code: "GENERATION_NOT_SEALED".to_string(),
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
        std::fs::remove_dir_all(&generation_dir).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: reclaim sealed generation {}: {error}",
                generation_dir.display()
            ))
        })?;
        if let Some(parent) = generation_dir.parent() {
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| {
                    CoreError::Storage(format!(
                        "lexical: fsync pair directory {} after reclaim: {error}",
                        parent.display()
                    ))
                })?;
        }
        self.invalidate_regex_match_cache_generation(&key)?;
        Ok(SealedGenerationReclaimOutcomeV1::Reclaimed { bytes })
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
            if !generation_dir.is_dir() || !lexical_sealed_identity_path(&generation_dir).exists() {
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
                    code: "GENERATION_IDENTITY_SCOPE_MISMATCH".to_string(),
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
