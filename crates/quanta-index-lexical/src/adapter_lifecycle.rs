//! A sealed generation's lifecycle ports: validation, scrub, quarantine, discard and reclaim.

use crate::generation_dir::{
    generation_tree_bytes_observed, is_writer_lock_entry, sync_generation_directory,
};
use crate::index_store::{
    read_lexical_sealed_identity, sealed_identity_entry_present, validate_lexical_sealed_identity,
};
use crate::inventory::{discard_quarantined_directory, inventory_sealed_generations};
use crate::sealed_generation::{
    DiscardingVisitor, ScrubStepResult, last_completed_scrub, quarantine_seal_failure_at,
    quarantined_by_scrub, quarantined_by_scrub_at, scrub_step, seal_failure_quarantine_reason,
    walk_sealed_generation, walk_sealed_generation_at,
};
use crate::{GenKey, LexicalAdapter};
use quanta_index_contract::{
    GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
};
use quanta_index_core::domains::generation::{
    GenerationStorageKeyV1, IncompleteGenerationDiscardOutcomeV1, IncompleteGenerationDiscardPort,
    QuarantinedGenerationV1, SealedGenerationBytesV1, SealedGenerationInventoryV1,
    SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort,
};
use quanta_index_core::{
    CoreError, DiagnosticContextV1, DiagnosticStageV1, DoorFindingOutcome,
    DoorFindingQuarantinePort, FinishedReclaims, GenerationIdentityValidatePort,
    GenerationNamespaceReadPort, IntegrityScrubBudgetV1, IntegrityScrubCandidateV1,
    IntegrityScrubCursorV1, IntegrityScrubPort, IntegrityScrubReportV1, QuarantineDiscardOutcomeV1,
    QuarantinedGenerationDiscardPort, SealedGenerationIdentityProbePort, SealedGenerationScanPort,
    reclaim_directory, unique_inode_tree_bytes_for_roots_below_track,
    unique_inode_tree_bytes_in_track,
};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;

impl GenerationNamespaceReadPort for LexicalAdapter {
    fn generation_namespace_is_empty(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<bool, CoreError> {
        let _lifecycle = self.directory_lifecycle_read_guard()?;
        let family = self
            .state_root
            .join(GenerationStorageKeyV1::for_repo_revision(repo_id, revision_id).as_str());
        for path in [&self.state_root, &family] {
            match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_dir() => {}
                Ok(_) => return Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
                Err(error) => {
                    return Err(CoreError::Storage(format!(
                        "lexical: inspect generation namespace {}: {error}",
                        path.display(),
                    )));
                }
            }
        }
        let mut entries = std::fs::read_dir(&family).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: list generation namespace {}: {error}",
                family.display(),
            ))
        })?;
        match entries.next() {
            None => Ok(true),
            Some(Ok(_)) => Ok(false),
            Some(Err(error)) => Err(CoreError::Storage(format!(
                "lexical: read generation namespace {}: {error}",
                family.display(),
            ))),
        }
    }
}

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
        let _verified = walk_sealed_generation(
            &generation_dir,
            &observed,
            &mut DiscardingVisitor,
            None,
            &crate::lexical_store::ObjectStore::open_existing(&self.state_root)?,
        )?;
        sync_generation_directory(&generation_dir)
    }

    fn validate_generation_identity_with_owner(
        &self,
        candidate: &GenerationSnapshot,
        owner: &mut quanta_index_core::PublicationValidationOwner,
    ) -> Result<(), CoreError> {
        let (generation_dir, observed) =
            self.sealed_generation_dir_for(candidate, "publication identity validator")?;
        let proofs = owner
            .proof_state::<crate::sealed_generation::publication_proof::PublicationProofs>(
                SearchPlaneTrackKind::Lexical,
            )?;
        let _verified = crate::sealed_generation::walk_sealed_generation_reusing_proofs(
            &generation_dir,
            &observed,
            None,
            proofs,
            None,
            None,
            &crate::lexical_store::ObjectStore::open_existing(&self.state_root)?,
        )?;
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
            Ok(None) | Err(crate::inventory::InventoryGenerationError::Quarantined(_)) => {
                return Ok(false);
            }
            Err(crate::inventory::InventoryGenerationError::Infrastructure(error)) => {
                return Err(error);
            }
        };
        if observed != *candidate {
            return Ok(false);
        }
        match quarantined_by_scrub(&dir) {
            Ok(None) => Ok(true),
            Err(CoreError::Storage(error)) => Err(CoreError::Storage(error)),
            Ok(Some(_)) | Err(_) => Ok(false),
        }
    }
}

impl LexicalAdapter {
    fn scrub_with_fence(
        &self,
        generation: &GenerationSnapshot,
        cursor: Option<IntegrityScrubCursorV1>,
        budget: IntegrityScrubBudgetV1,
        before_quarantine: &dyn Fn(&GenerationSnapshot) -> Result<(), CoreError>,
    ) -> Result<IntegrityScrubReportV1, CoreError> {
        let key = GenKey {
            repo_id: generation.repo_id.clone(),
            revision_id: generation.revision_id.clone(),
            generation: generation.manifest_generation,
        };
        let _mutation = self.generation_mutation_guard(&key)?;
        let lifecycle = self.directory_lifecycle_read_guard()?;
        let (generation_dir, observed) = self.sealed_generation_dir_for(generation, "scrub")?;
        let mut progress = self.scrub_progress.lock().map_err(|error| {
            CoreError::Storage(format!("lexical scrub progress poisoned: {error}"))
        })?;
        let start = progress.start_or_resume(&observed, cursor)?;
        let mut shared = self.shared_scrub_progress.lock().map_err(|error| {
            CoreError::Storage(format!("lexical shared scrub progress poisoned: {error}"))
        })?;
        let step = scrub_step(
            &self.state_root,
            &generation_dir,
            &observed,
            start,
            budget,
            before_quarantine,
            &mut shared,
        );
        let fault = match &step {
            Ok(ScrubStepResult::Report(report))
                if !matches!(
                    &report.outcome,
                    quanta_index_core::IntegrityScrubOutcomeV1::Corrupt { .. }
                ) =>
            {
                None
            }
            Ok(_) | Err(_) => shared.fault_reference()?,
        };
        drop(shared);
        drop(progress);
        let origin_receipt = if let Some(fault) = fault {
            drop(lifecycle);
            let _exclusive = self.directory_lifecycle_guard()?;
            self.quarantine_shared_references(&fault, &observed, before_quarantine)?
        } else {
            None
        };
        let report = match step {
            Ok(ScrubStepResult::Report(report)) => Ok(report),
            Ok(ScrubStepResult::DeferredSharedFault {
                files_verified,
                bytes_read,
            }) => {
                let quarantined = origin_receipt.ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical shared fault did not quarantine its origin".into(),
                    )
                })?;
                Ok(IntegrityScrubReportV1 {
                    generation: observed.clone(),
                    files_verified,
                    bytes_read,
                    outcome: quanta_index_core::IntegrityScrubOutcomeV1::Corrupt { quarantined },
                })
            }
            Err(error) => Err(error),
        };
        if let Ok(report) = &report {
            self.scrub_progress
                .lock()
                .map_err(|error| {
                    CoreError::Storage(format!("lexical scrub progress poisoned: {error}"))
                })?
                .record(&observed, &report.outcome)?;
        }
        report
    }

    fn quarantine_shared_references(
        &self,
        fault: &crate::lexical_store::ObjectRef,
        origin: &GenerationSnapshot,
        before_quarantine: &dyn Fn(&GenerationSnapshot) -> Result<(), CoreError>,
    ) -> Result<Option<QuarantinedGenerationV1>, CoreError> {
        let store = crate::lexical_store::ObjectStore::open_existing(&self.state_root)?;
        let inventory = inventory_sealed_generations(&self.state_root)?;
        let mut referencing = Vec::new();
        let mut reachability_error = None;
        for sealed in &inventory.sealed {
            let reaches = crate::lexical_gc::protected_roots(&sealed.path).and_then(|roots| {
                crate::lexical_gc::generation_references_object(&store, &roots, fault, None)
            });
            match reaches {
                Ok(true) => {
                    if let Err(error) = referencing.try_reserve(1) {
                        reachability_error = Some(CoreError::Storage(format!(
                            "lexical: reserve shared quarantine references: {error}"
                        )));
                        break;
                    }
                    referencing.push(sealed);
                }
                Ok(false) => {}
                Err(error) => {
                    reachability_error = Some(error);
                    break;
                }
            }
        }

        // A failed reachability proof cannot exclude any inventoried generation.
        // Fence the complete inventory before returning the proof error.
        let mut fence_error = None;
        if reachability_error.is_some() {
            for sealed in &inventory.sealed {
                if let Err(error) = before_quarantine(&sealed.identity)
                    && fence_error.is_none()
                {
                    fence_error = Some(error);
                }
            }
        } else {
            for sealed in &referencing {
                if let Err(error) = before_quarantine(&sealed.identity)
                    && fence_error.is_none()
                {
                    fence_error = Some(error);
                }
            }
        }
        if let Some(error) = fence_error.or(reachability_error) {
            return Err(error);
        }

        // No receipt is published until every known reference is fenced.
        let mut origin_receipt = None;
        let mut receipt_error = None;
        for sealed in referencing {
            let receipt = crate::sealed_generation::open_generation_dir_nofollow(&sealed.path)
                .map_err(|error| {
                    CoreError::Storage(format!("lexical: pin shared quarantine: {error}"))
                })
                .and_then(|root| {
                    crate::sealed_generation::quarantine_content_corrupt_at(
                        &root,
                        &sealed.path,
                        &format!("shared lexical object corruption: {:x?}", fault.sha256),
                    )
                });
            match receipt {
                Ok(quarantined) if sealed.identity == *origin => {
                    origin_receipt = Some(quarantined);
                }
                Err(error) if receipt_error.is_none() => receipt_error = Some(error),
                Ok(_) | Err(_) => {}
            }
        }
        if let Some(error) = receipt_error {
            return Err(error);
        }
        Ok(origin_receipt)
    }

    /// Caller holds this generation's mutation stripe and directory fence.
    /// The sealed identity and compact commitments independently own the roots;
    /// a crash after seal cannot leave its adoption journal live indefinitely.
    pub(crate) fn settle_published_adoptions(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<(), CoreError> {
        let (directory, identity) =
            self.sealed_generation_dir_for(candidate, "settle published adoption")?;
        let opened = crate::sealed_generation::open_generation_dir_nofollow(&directory).map_err(
            |error| CoreError::Storage(format!("lexical: pin published adoption: {error}")),
        )?;
        let manifest = crate::sealed_generation::read_bound_manifest_at(
            &directory,
            &opened,
            &identity.manifest_digest,
        )?;
        let coverage = manifest.source_coverage.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("lexical sealed publication lacks source event".into())
        })?;
        let event = crate::sealed_generation::coverage::read_committed_publication_at(
            &opened, &directory, coverage, &identity,
        )?;
        let (file, native) = crate::lexical_publication::read_committed_roots(&directory)?;
        let mut roots = file.object_roots();
        roots.extend(native.root_objects());
        let store = crate::lexical_store::ObjectStore::open_existing(&self.state_root)?;
        let _settled = store.finish_target_attempts(
            Sha256::digest(identity.repo_id.as_str().as_bytes()).into(),
            Sha256::digest(identity.revision_id.as_str().as_bytes()).into(),
            identity.manifest_generation.get(),
            Sha256::digest(crate::channel_payloads::encode_cbor(
                &event,
                "lexical source event",
            )?)
            .into(),
            &roots,
        )?;
        Ok(())
    }

    /// Call only after exact target absence and writer/lifecycle settlement.
    fn discard_target_adoptions(&self, key: &GenKey) -> Result<(), CoreError> {
        match std::fs::symlink_metadata(self.state_root.join("lexical-object-pool")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: inspect adoption pool: {error}"
                )));
            }
            Ok(_) => {}
        }
        let store = crate::lexical_store::ObjectStore::open_existing(&self.state_root)?;
        let _retired = store.discard_target_attempts(
            Sha256::digest(key.repo_id.as_str().as_bytes()).into(),
            Sha256::digest(key.revision_id.as_str().as_bytes()).into(),
            key.generation.get(),
        )?;
        Ok(())
    }
}

impl IntegrityScrubPort for LexicalAdapter {
    fn collect_shared_objects_step(
        &self,
        max_work_units: u32,
    ) -> Result<quanta_index_core::SharedObjectCollectionStepV1, CoreError> {
        use quanta_index_core::SharedObjectCollectionStepV1;
        if max_work_units == 0 {
            return Err(CoreError::InvalidContract(
                "shared collection requires positive work units".into(),
            ));
        }
        let mut collector = self
            .shared_gc
            .lock()
            .map_err(|error| CoreError::Storage(format!("lexical shared GC poisoned: {error}")))?;
        if collector.is_none() {
            // Only targets with an outstanding journal need sealed recovery.
            // Acquire each mutation stripe before its directory fence, as ingest
            // and discard do, then capture a fresh GC inventory afterwards.
            let pending =
                match std::fs::symlink_metadata(self.state_root.join("lexical-object-pool")) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
                    Err(error) => {
                        return Err(CoreError::Storage(format!(
                            "lexical: inspect recovery pool: {error}"
                        )));
                    }
                    Ok(_) => crate::lexical_store::ObjectStore::open_existing(&self.state_root)?
                        .pending_target_scopes()?,
                };
            if !pending.is_empty() {
                let candidates = {
                    let _inventory_fence = self.directory_lifecycle_read_guard()?;
                    inventory_sealed_generations(&self.state_root)?.sealed
                };
                for candidate in candidates {
                    let identity = &candidate.identity;
                    let scope = (
                        <[u8; 32]>::from(Sha256::digest(identity.repo_id.as_str().as_bytes())),
                        <[u8; 32]>::from(Sha256::digest(identity.revision_id.as_str().as_bytes())),
                        identity.manifest_generation.get(),
                    );
                    if !pending.contains(&scope) {
                        continue;
                    }
                    let key = GenKey {
                        repo_id: identity.repo_id.clone(),
                        revision_id: identity.revision_id.clone(),
                        generation: identity.manifest_generation,
                    };
                    let _mutation = self.generation_mutation_guard(&key)?;
                    let _namespace = self.directory_lifecycle_read_guard()?;
                    if sealed_identity_entry_present(&candidate.path)? {
                        self.settle_published_adoptions(identity)?;
                        return Ok(quanta_index_core::SharedObjectCollectionStepV1::Paused);
                    }
                }
            }
            let lifecycle = self.directory_lifecycle_read_guard()?;
            match std::fs::symlink_metadata(self.state_root.join("lexical-object-pool")) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let inventory = inventory_sealed_generations(&self.state_root)?;
                    if inventory.sealed.is_empty() && inventory.quarantined.is_empty() {
                        return Ok(SharedObjectCollectionStepV1::Completed);
                    }
                }
                Err(error) => {
                    return Err(CoreError::Storage(format!(
                        "lexical: inspect GC pool: {error}"
                    )));
                }
                Ok(_) => {}
            }
            let store = crate::lexical_store::ObjectStore::open_existing(&self.state_root)?;
            let epoch = store.gc_epoch()?;
            let inventory = inventory_sealed_generations(&self.state_root)?;
            let retained = inventory
                .sealed
                .into_iter()
                .map(|entry| entry.path)
                .chain(inventory.quarantined.into_iter().map(|entry| entry.path))
                .collect();
            drop(lifecycle);
            *collector = Some(crate::lexical_gc::SharedGcProgress::start(
                &self.state_root,
                retained,
                epoch,
            )?);
        }
        let result = collector
            .as_mut()
            .ok_or_else(|| CoreError::InvalidContract("shared GC progress absent".into()))?
            .step(
                usize::try_from(max_work_units.min(256)).map_err(|error| {
                    CoreError::InvalidContract(format!("GC work width: {error}"))
                })?,
                None,
            );
        let outcome = match result {
            Ok(crate::lexical_gc::SharedGcStep::Paused) => Ok(SharedObjectCollectionStepV1::Paused),
            Ok(crate::lexical_gc::SharedGcStep::Completed { .. }) => {
                drop(collector.take());
                Ok(SharedObjectCollectionStepV1::Completed)
            }
            Ok(crate::lexical_gc::SharedGcStep::Stale) => {
                drop(collector.take());
                Ok(SharedObjectCollectionStepV1::Stale)
            }
            Err(error) => {
                drop(collector.take());
                Err(error)
            }
        };
        drop(collector);
        outcome
    }
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
        self.scrub_with_fence(generation, cursor, budget, &|_| Ok(()))
    }

    fn scrub_with_quarantine_fence(
        &self,
        generation: &GenerationSnapshot,
        cursor: Option<IntegrityScrubCursorV1>,
        budget: IntegrityScrubBudgetV1,
        before_quarantine: &dyn Fn(&GenerationSnapshot) -> Result<(), CoreError>,
    ) -> Result<IntegrityScrubReportV1, CoreError> {
        self.scrub_with_fence(generation, cursor, budget, before_quarantine)
    }
}

impl DoorFindingQuarantinePort for LexicalAdapter {
    fn quarantine_door_finding(
        &self,
        generation: &GenerationSnapshot,
    ) -> Result<DoorFindingOutcome, CoreError> {
        self.quarantine_door_finding_with_fence(generation, &|_| Ok(()))
    }

    fn quarantine_door_finding_with_fence(
        &self,
        generation: &GenerationSnapshot,
        before_quarantine: &dyn Fn(&GenerationSnapshot) -> Result<(), CoreError>,
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
            before_quarantine(&observed)?;
            return Ok(DoorFindingOutcome::Quarantined { quarantined });
        }
        let store =
            crate::lexical_store::ObjectStore::open_existing(&self.state_root)?.metered_clone();
        match walk_sealed_generation_at(
            &root,
            &generation_dir,
            &observed,
            &mut DiscardingVisitor,
            None,
            None,
            &store,
        ) {
            Ok(_verified) => Ok(DoorFindingOutcome::NotReproduced),
            Err(error) => match seal_failure_quarantine_reason(&error) {
                Some(reason) => {
                    if let Some(fault) = store.fault_reference()? {
                        let quarantined = self
                            .quarantine_shared_references(&fault, &observed, before_quarantine)?
                            .ok_or_else(|| {
                                CoreError::InvalidContract(
                                    "lexical shared door fault did not quarantine its origin"
                                        .into(),
                                )
                            })?;
                        return Ok(DoorFindingOutcome::Quarantined { quarantined });
                    }
                    before_quarantine(&observed)?;
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
        let retired_target = match read_lexical_sealed_identity(&entry.path) {
            Ok(identity) => {
                let key = GenKey {
                    repo_id: identity.repo_id,
                    revision_id: identity.revision_id,
                    generation: identity.manifest_generation,
                };
                (self.index_path(&key) == entry.path).then_some(key)
            }
            // An unreadable quarantined identity grants no authority to retire
            // adoption journals. The separately proved quarantine path may
            // still be removed; its journals remain protected for recovery.
            Err(_unreadable_quarantined_identity) => None,
        };
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
        if let Some(key) = retired_target
            && matches!(std::fs::symlink_metadata(&entry.path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        {
            self.discard_target_adoptions(&key)?;
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
        let _lifecycle = self.directory_lifecycle_guard()?;
        let generation_dir = self.index_path(&key);
        let mut writers = self
            .writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
        let absent = match std::fs::symlink_metadata(&generation_dir) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: inspect incomplete generation: {error}"
                )));
            }
            Ok(_) => false,
        };
        if absent {
            let _stale_writer = writers.remove(&key);
            drop(writers);
            self.discard_target_adoptions(&key)?;
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
        reclaim_directory(
            &self.state_root,
            &generation_dir,
            &format!(
                "incomplete-{}",
                GenerationStorageKeyV1::for_repo_revision(&key.repo_id, &key.revision_id)
                    .reclaim_entry_name(key.generation)
            ),
        )?;
        drop(writer_guard);
        drop(writer);
        drop(writers);
        self.discard_target_adoptions(&key)?;
        self.invalidate_regex_match_cache_generation(&key)?;
        Ok(IncompleteGenerationDiscardOutcomeV1::Discarded)
    }
}

impl SealedGenerationReclaimPort for LexicalAdapter {
    fn reclaim_sealed_generation_with_settlement(
        &self,
        retired: &GenerationSnapshot,
        on_absent: &dyn Fn() -> Result<(), CoreError>,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<SealedGenerationReclaimOutcomeV1, CoreError> {
        crate::causal_profile::observe_result(
            diagnostic,
            DiagnosticStageV1::Compaction,
            |diagnostic| {
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
                        self.discard_target_adoptions(&key)?;
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
                let bytes =
                    generation_tree_bytes_observed(&self.state_root, &generation_dir, diagnostic)?;
                // A sealed generation has no live writer, but a stale handle from an
                // earlier attempt must not outlive the directory.
                {
                    let mut writers = self.writers.lock().map_err(|err| {
                        CoreError::Storage(format!("lexical writers poisoned: {err}"))
                    })?;
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
                self.discard_target_adoptions(&key)?;
                self.invalidate_regex_match_cache_generation(&key)?;
                Ok(SealedGenerationReclaimOutcomeV1::Reclaimed { bytes })
            },
        )
    }

    fn finish_interrupted_reclaims(
        &self,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<FinishedReclaims, CoreError> {
        crate::causal_profile::observe_result(
            diagnostic,
            DiagnosticStageV1::Compaction,
            |diagnostic| {
                let _lifecycle = self.directory_lifecycle_guard()?;
                quanta_index_core::finish_interrupted_reclaims(&self.state_root, diagnostic)
            },
        )
    }

    fn sealed_generations_for_pair(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<Vec<GenerationSnapshot>, CoreError> {
        crate::causal_profile::observe_result(
            diagnostic,
            DiagnosticStageV1::RetentionRootSnapshot,
            |_diagnostic| {
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
                    if !generation_dir.is_dir() || !sealed_identity_entry_present(&generation_dir)?
                    {
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
            },
        )
    }

    /// Exact bytes of the named generation directories plus their shared
    /// immutable object closure. Each inode and object digest counts once
    /// across the selected generations.
    fn measure_sealed_generations(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        crate::causal_profile::observe_result(
            diagnostic,
            DiagnosticStageV1::RetentionMeasure,
            |diagnostic| {
                let _namespace = self.directory_lifecycle_read_guard()?;
                let roots: Vec<_> = generations
                    .iter()
                    .map(|generation| {
                        self.index_path(&GenKey {
                            repo_id: repo_id.clone(),
                            revision_id: revision_id.clone(),
                            generation: *generation,
                        })
                    })
                    .collect();
                let (bytes, present) = unique_inode_tree_bytes_for_roots_below_track(
                    &self.state_root,
                    &roots,
                    &is_writer_lock_entry,
                    diagnostic,
                )
                .map_err(|err| {
                    CoreError::Storage(format!(
                        "lexical: measure sealed generations of repo={} revision={}: {err}",
                        repo_id.as_str(),
                        revision_id.as_str()
                    ))
                })?;
                let mut absent = BTreeSet::new();
                let mut object_roots = Vec::new();
                for ((generation, directory), present) in
                    generations.iter().zip(&roots).zip(present)
                {
                    if !present {
                        let _inserted = absent.insert(*generation);
                        continue;
                    }
                    let sealed_roots = crate::lexical_gc::protected_roots(directory)?;
                    object_roots
                        .try_reserve(sealed_roots.len())
                        .map_err(|error| {
                            CoreError::Storage(format!("lexical retention root reserve: {error}"))
                        })?;
                    object_roots.extend(sealed_roots);
                }
                let object_bytes = if object_roots.is_empty() {
                    0
                } else {
                    let store = crate::lexical_store::ObjectStore::open_existing(&self.state_root)?;
                    crate::lexical_gc::measure_unique_closure_bytes(
                        &store,
                        object_roots,
                        diagnostic,
                    )?
                };
                let bytes = bytes.checked_add(object_bytes).ok_or_else(|| {
                    CoreError::InvalidContract("lexical retention pair byte total overflow".into())
                })?;
                Ok(SealedGenerationBytesV1 { bytes, absent })
            },
        )
    }

    /// Physical lexical track bytes, including shared pool, quarantine,
    /// unsealed attempts and reclaim-area files until they are actually gone.
    fn measure_track_physical_bytes(
        &self,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<u64, CoreError> {
        crate::causal_profile::observe_result(
            diagnostic,
            DiagnosticStageV1::PhysicalInodeCensus,
            |diagnostic| {
                match std::fs::symlink_metadata(&self.state_root) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
                    Err(error) => {
                        return Err(CoreError::Storage(format!(
                            "lexical: inspect physical track {}: {error}",
                            self.state_root.display()
                        )));
                    }
                    Ok(_) => {}
                }
                unique_inode_tree_bytes_in_track(
                    &self.state_root,
                    &is_writer_lock_entry,
                    &quanta_index_core::RequestBudgetV1::unbounded(),
                    diagnostic,
                )
            },
        )
    }
}

#[cfg(test)]
mod generation_namespace_tests {
    use super::*;

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "Assertions independently verify empty and occupied namespace fixtures"
    )]
    fn first_pair_probe_sees_incomplete_entries_and_symlink_family()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let lexical = LexicalAdapter::with_state_root(root.path().join("lexical"));
        let repo = RepoId::new("repo").expect("fixture repo");
        let revision = RevisionId::new("revision").expect("fixture revision");
        assert!(lexical.generation_namespace_is_empty(&repo, &revision)?);
        std::fs::create_dir(&lexical.state_root)?;
        let family = lexical
            .state_root
            .join(GenerationStorageKeyV1::for_repo_revision(&repo, &revision).as_str());
        std::fs::create_dir(&family)?;
        assert!(lexical.generation_namespace_is_empty(&repo, &revision)?);
        std::fs::write(family.join("g1-incomplete"), b"unsealed")?;
        assert!(!lexical.generation_namespace_is_empty(&repo, &revision)?);
        std::fs::remove_file(family.join("g1-incomplete"))?;
        std::fs::remove_dir(&family)?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.path(), &family)?;
            assert!(!lexical.generation_namespace_is_empty(&repo, &revision)?);
        }
        Ok(())
    }
}
