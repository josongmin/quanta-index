//! Source publication state is mutated only through the repository envelope.

use quanta_index_contract::{
    GenerationPin, GenerationSnapshot, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneErrorCodeV2, SourcePublicationEvent,
};
use quanta_index_core::{
    CoreError, IdempotencyCatalogPort, LexicalDocumentNamespaceV1, LexicalDocumentVersionRangeV1,
    LexicalDocumentVersionReservationV1, OperationInspectV1, SourceEventAllocationOutcomeV1,
    SourceEventAllocationRequestV1, SourceEventAllocationV1, SourceEventBindingV1,
    SourceEventPhaseV1, SourceEventRecordV1, SourceEventReservationV1, SourceEventStateV1,
    SourcePublicationCatalogPort,
};

use super::repository_envelope::{MAX_EVENTS, MAX_STREAMS, capacity, corrupt};
use super::{ActivationCatalog, CatalogState};

fn conflict(message: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::BatchDigestConflict,
        message: format!("source publication event conflict: {message}"),
    }
}

fn existing(
    state: &CatalogState,
    repo: &RepoId,
    event: &SourcePublicationEvent,
) -> Result<Option<SourceEventRecordV1>, CoreError> {
    event
        .validate()
        .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
    let record = state.histories.get(repo).and_then(|history| {
        history
            .records
            .get(&(event.stream_id.clone(), event.event_id.clone()))
    });
    if let Some(record) = record
        && record.allocation.request.event != *event
    {
        return Err(conflict(
            "event identity reused with different payload or expected base",
        ));
    }
    Ok(record.cloned())
}

fn bound(
    record: &SourceEventRecordV1,
) -> Result<(&SourceEventBindingV1, SourceEventPhaseV1), CoreError> {
    match &record.state {
        SourceEventStateV1::Allocated => Err(CoreError::NotReady(
            "source event is allocated but its final digest and journal key are not bound".into(),
        )),
        SourceEventStateV1::Bound { binding, phase } => Ok((binding, *phase)),
    }
}

impl ActivationCatalog {
    fn admit_generation_number_v1(
        &self,
        entries: &CatalogState,
        repo: &RepoId,
        revision: &RevisionId,
        explicit: Option<ManifestGeneration>,
    ) -> Result<ManifestGeneration, CoreError> {
        let current = entries
            .histories
            .get(repo)
            .and_then(|history| history.high_water.get(revision));
        let next = if let Some(water) = current {
            water
                .get()
                .checked_add(1)
                .ok_or_else(|| capacity("source generation counter exhausted"))?
        } else {
            if super::active_search_corpus_generation_v1(&entries.roots, repo, revision).is_some()
                || !self.first_generation_namespace_is_empty_v1(repo, revision)?
            {
                return Err(CoreError::NotReady("first source generation requires empty lexical and semantic namespaces and no active root; rebuild the pair".into()));
            }
            1
        };
        let generation = explicit.unwrap_or_else(|| ManifestGeneration::new(next));
        if generation.get() == 0 || generation.get() < next {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::SearchCorpusGenerationConflict,
                message: "source generation must advance the durable high-water".into(),
            });
        }
        Ok(generation)
    }

    fn check_new_event_admission_v1(
        &self,
        entries: &CatalogState,
        request: &SourceEventAllocationRequestV1,
        generation: ManifestGeneration,
    ) -> Result<(), CoreError> {
        let repo = &request.repo_id;
        let revision = &request.revision_id;
        if super::active_search_corpus_generation_v1(&entries.roots, repo, revision)
            .is_some_and(|active| active.manifest_generation() >= generation)
        {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::SearchCorpusGenerationConflict,
                message: "source publication target must advance the active generation".into(),
            });
        }
        let history = entries.histories.get(repo);
        let stream = history.and_then(|history| history.streams.get(&request.event.stream_id));
        if stream.and_then(|stream| stream.pending.as_ref()).is_some() {
            return Err(CoreError::NotReady("source publication stream has an unresolved reservation; reconcile its original journal".into()));
        }
        if stream.and_then(|stream| stream.active.as_ref())
            != request.event.expected_base_event_id.as_ref()
        {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::DeltaBaseConflict,
                message: "source event expected base differs from the active stream high-water"
                    .into(),
            });
        }
        if history.is_some_and(|history| {
            history.records.len() >= MAX_EVENTS
                || (stream.is_none() && history.streams.len() >= MAX_STREAMS)
        }) {
            return Err(capacity(
                "retained event or stream limit; identities cannot be evicted",
            ));
        }
        Ok(())
    }
}

impl SourcePublicationCatalogPort for ActivationCatalog {
    fn reserve_lexical_document_versions(
        &self,
        binding: &SourceEventBindingV1,
        file_count: u64,
        native_text_count: u64,
    ) -> Result<LexicalDocumentVersionReservationV1, CoreError> {
        binding.validate()?;
        let repo = &binding.target.repo_id;
        let key = (
            binding.event.stream_id.clone(),
            binding.event.event_id.clone(),
        );
        let mutation = self.lock_mutation()?;
        let entries = self
            .entries
            .read()
            .map_err(|error| corrupt(error.to_string()))?;
        self.ensure_durability_certain_v1()?;
        let record = existing(&entries, repo, &binding.event)?.ok_or_else(|| {
            CoreError::NotReady("lexical IDs require the original source-event binding".into())
        })?;
        let (original, phase) = bound(&record)?;
        if original != binding || record.allocation.root_incarnation != self.root_incarnation_v1() {
            return Err(conflict(
                "lexical IDs require the original target and current root incarnation",
            ));
        }
        let history = entries
            .histories
            .get(repo)
            .ok_or_else(|| corrupt("missing source history"))?;
        if let Some(reservation) = history.lexical_reservations.get(&key) {
            if reservation.file.count != file_count
                || reservation.native_text.count != native_text_count
            {
                return Err(conflict(
                    "lexical ID retry changed its original document counts",
                ));
            }
            return Ok(reservation.clone());
        }
        if phase != SourceEventPhaseV1::Pending {
            return Err(corrupt(
                "accepted source event lost its lexical ID reservation",
            ));
        }
        let mut next = entries.repository(repo);
        let history = next
            .histories
            .get_mut(repo)
            .ok_or_else(|| corrupt("missing source history"))?;
        let incarnation = record.allocation.root_incarnation;
        let mut reserve = |tag, namespace, count| -> Result<_, CoreError> {
            let water = history
                .lexical_high_water
                .get(&(incarnation, tag))
                .copied()
                .unwrap_or(0);
            let end = water
                .checked_add(count)
                .ok_or_else(|| capacity("lexical document counter exhausted"))?;
            let range = LexicalDocumentVersionRangeV1 {
                first: if count == 0 {
                    0
                } else {
                    water
                        .checked_add(1)
                        .ok_or_else(|| capacity("lexical document counter exhausted"))?
                },
                count,
                scope_sha256: LexicalDocumentVersionRangeV1::scope_digest(
                    &incarnation,
                    repo,
                    namespace,
                ),
            };
            range.validate()?;
            let _prior = history.lexical_high_water.insert((incarnation, tag), end);
            Ok(range)
        };
        let reservation = LexicalDocumentVersionReservationV1 {
            allocation: record.allocation,
            file: reserve(1, LexicalDocumentNamespaceV1::File, file_count)?,
            native_text: reserve(2, LexicalDocumentNamespaceV1::NativeText, native_text_count)?,
        };
        reservation.validate()?;
        let _prior = history
            .lexical_reservations
            .insert(key, reservation.clone());
        drop(entries);
        self.persist_repository(&mutation, repo, next)?;
        Ok(reservation)
    }

    fn require_reserved_generation(&self, pin: &GenerationPin) -> Result<(), CoreError> {
        let entries = self
            .entries
            .read()
            .map_err(|error| corrupt(error.to_string()))?;
        self.ensure_durability_certain_v1()?;
        let owned = entries
            .histories
            .get(&pin.repo_id)
            .into_iter()
            .flat_map(|history| history.records.values())
            .any(|record| {
                record.allocation.request.revision_id == pin.revision_id
                    && record.allocation.generation == pin.manifest_generation
                    && record.allocation.root_incarnation == self.root_incarnation_v1()
            });
        drop(entries);
        if !owned {
            return Err(CoreError::NotReady(
                "lexical overlay target has no source generation reservation in the current root incarnation".into(),
            ));
        }
        Ok(())
    }

    fn inspect_source_event(
        &self,
        repo: &RepoId,
        event: &SourcePublicationEvent,
    ) -> Result<Option<SourceEventRecordV1>, CoreError> {
        let entries = self
            .entries
            .read()
            .map_err(|error| corrupt(error.to_string()))?;
        self.ensure_durability_certain_v1()?;
        existing(&entries, repo, event)
    }

    fn allocate_source_event(
        &self,
        request: &SourceEventAllocationRequestV1,
    ) -> Result<SourceEventAllocationOutcomeV1, CoreError> {
        request.validate()?;
        let repo = &request.repo_id;
        let mutation = self.lock_mutation()?;
        let entries = self
            .entries
            .read()
            .map_err(|error| corrupt(error.to_string()))?;
        self.ensure_durability_certain_v1()?;
        if let Some(record) = existing(&entries, repo, &request.event)? {
            if record.allocation.request.revision_id != request.revision_id {
                return Err(conflict(
                    "allocated event identity reused under a different revision",
                ));
            }
            return Ok(SourceEventAllocationOutcomeV1::Existing(record));
        }
        let generation =
            self.admit_generation_number_v1(&entries, repo, &request.revision_id, None)?;
        self.check_new_event_admission_v1(&entries, request, generation)?;
        let record = SourceEventRecordV1 {
            allocation: SourceEventAllocationV1 {
                request: request.clone(),
                generation,
                root_incarnation: self.root_incarnation_v1(),
            },
            state: SourceEventStateV1::Allocated,
        };
        record.validate()?;
        let mut next = entries.repository(repo);
        let history = next.histories.entry(repo.clone()).or_default();
        let _previous = history
            .high_water
            .insert(request.revision_id.clone(), generation);
        history
            .streams
            .entry(request.event.stream_id.clone())
            .or_default()
            .pending = Some(request.event.event_id.clone());
        let _previous = history.records.insert(
            (
                request.event.stream_id.clone(),
                request.event.event_id.clone(),
            ),
            record.clone(),
        );
        drop(entries);
        self.persist_repository(&mutation, repo, next)?;
        Ok(SourceEventAllocationOutcomeV1::Allocated(record))
    }

    fn reserve_source_event(
        &self,
        binding: &SourceEventBindingV1,
    ) -> Result<SourceEventReservationV1, CoreError> {
        binding.validate()?;
        super::repository_envelope::bound_manifest_token(&binding.target.manifest_digest)?;
        let repo = &binding.target.repo_id;
        let mutation = self.lock_mutation()?;
        let entries = self
            .entries
            .read()
            .map_err(|error| corrupt(error.to_string()))?;
        self.ensure_durability_certain_v1()?;
        if let Some(record) = existing(&entries, repo, &binding.event)? {
            match &record.state {
                SourceEventStateV1::Bound {
                    binding: original, ..
                } => {
                    // Historical bound replay resolves the original event even if
                    // its caller names a newer revision or generation.
                    if record.allocation.request.revision_id == binding.target.revision_id
                        && original.as_ref() != binding
                    {
                        return Err(conflict(
                            "bound event replay differs from its original target or journal key",
                        ));
                    }
                    return Ok(SourceEventReservationV1::Existing(record));
                }
                SourceEventStateV1::Allocated => {
                    if record.allocation.request.revision_id != binding.target.revision_id
                        || record.allocation.generation != binding.target.manifest_generation
                    {
                        return Err(conflict("final target differs from original allocation"));
                    }
                    if record.allocation.root_incarnation != self.root_incarnation_v1() {
                        return Err(conflict(
                            "allocation belongs to a previous activation root incarnation",
                        ));
                    }
                    let mut next = entries.repository(repo);
                    let updated = next
                        .histories
                        .get_mut(repo)
                        .and_then(|history| {
                            history.records.get_mut(&(
                                binding.event.stream_id.clone(),
                                binding.event.event_id.clone(),
                            ))
                        })
                        .ok_or_else(|| {
                            corrupt("allocated event disappeared from repository snapshot")
                        })?;
                    updated.state = SourceEventStateV1::Bound {
                        binding: Box::new(binding.clone()),
                        phase: SourceEventPhaseV1::Pending,
                    };
                    updated.validate()?;
                    let updated = updated.clone();
                    drop(entries);
                    self.persist_repository(&mutation, repo, next)?;
                    return Ok(SourceEventReservationV1::Reserved(updated));
                }
            }
        }
        let generation = self.admit_generation_number_v1(
            &entries,
            repo,
            &binding.target.revision_id,
            Some(binding.target.manifest_generation),
        )?;
        let request = SourceEventAllocationRequestV1 {
            repo_id: repo.clone(),
            revision_id: binding.target.revision_id.clone(),
            event: binding.event.clone(),
        };
        self.check_new_event_admission_v1(&entries, &request, generation)?;
        let record = SourceEventRecordV1 {
            allocation: SourceEventAllocationV1 {
                request: request.clone(),
                generation,
                root_incarnation: self.root_incarnation_v1(),
            },
            state: SourceEventStateV1::Bound {
                binding: Box::new(binding.clone()),
                phase: SourceEventPhaseV1::Pending,
            },
        };
        record.validate()?;
        let mut next = entries.repository(repo);
        let history = next.histories.entry(repo.clone()).or_default();
        let _previous = history.high_water.insert(request.revision_id, generation);
        history
            .streams
            .entry(binding.event.stream_id.clone())
            .or_default()
            .pending = Some(binding.event.event_id.clone());
        let _previous = history.records.insert(
            (
                binding.event.stream_id.clone(),
                binding.event.event_id.clone(),
            ),
            record.clone(),
        );
        drop(entries);
        self.persist_repository(&mutation, repo, next)?;
        Ok(SourceEventReservationV1::Reserved(record))
    }

    fn reconcile_source_event(
        &self,
        repo: &RepoId,
        event: &SourcePublicationEvent,
        journal: &dyn IdempotencyCatalogPort,
    ) -> Result<SourceEventRecordV1, CoreError> {
        let record = self
            .inspect_source_event(repo, event)?
            .ok_or_else(|| CoreError::NotReady("source event has no durable reservation".into()))?;
        let (binding, _phase) = bound(&record)?;
        // No catalog lock is held while calling another authority. Recheck the
        // original binding under the mutation lock before advancing its phase.
        let OperationInspectV1::Committed {
            receipt,
            durable_sequence,
        } = journal.inspect(&binding.journal_key)?
        else {
            return Err(CoreError::NotReady("source event original journal has no committed stage receipt; reservation retained".into()));
        };
        // The journal persists the pre-terminal receipt before allocating its
        // sequence; `OperationInspectV1::durable_sequence` is the ledger's
        // authority, as on the dispatcher's existing recorded_at replay path.
        if receipt.generation != binding.target.manifest_generation
            || receipt.manifest_digest.as_ref() != Some(&binding.target.manifest_digest)
            || receipt.batch_digest != binding.journal_key.batch_digest
            || !receipt.sealed
            || receipt
                .semantic_content
                .as_ref()
                .is_none_or(|roots| !roots.is_canonical_v1())
            || durable_sequence == 0
        {
            return Err(corrupt(
                "original committed journal receipt does not bind the sealed source target",
            ));
        }
        let mutation = self.lock_mutation()?;
        let entries = self
            .entries
            .read()
            .map_err(|error| corrupt(error.to_string()))?;
        self.ensure_durability_certain_v1()?;
        let observed = existing(&entries, repo, event)?
            .ok_or_else(|| corrupt("reservation disappeared during reconciliation"))?;
        if observed.allocation != record.allocation || bound(&observed)?.0 != binding {
            return Err(corrupt(
                "original event binding changed during reconciliation",
            ));
        }
        if bound(&observed)?.1 != SourceEventPhaseV1::Pending {
            return Ok(observed);
        }
        let mut next = entries.repository(repo);
        let updated = next
            .histories
            .get_mut(repo)
            .and_then(|history| {
                history
                    .records
                    .get_mut(&(event.stream_id.clone(), event.event_id.clone()))
            })
            .ok_or_else(|| corrupt("reservation disappeared from repository snapshot"))?;
        if let SourceEventStateV1::Bound { phase, .. } = &mut updated.state {
            *phase = SourceEventPhaseV1::Staged;
        }
        let updated = updated.clone();
        drop(entries);
        self.persist_repository(&mutation, repo, next)?;
        Ok(updated)
    }
}

/// Called only after the lifecycle opened and proved the lexical generation.
/// The event is obtained from that proven reader's sealed coverage artifact.
pub(super) fn activate_event(
    state: &mut CatalogState,
    target: &GenerationSnapshot,
    event: Option<&SourcePublicationEvent>,
) -> Result<(), CoreError> {
    if state.histories.get(&target.repo_id).is_some_and(|history| {
        history.records.values().any(|record| {
            !matches!(
                record.state,
                SourceEventStateV1::Bound {
                    phase: SourceEventPhaseV1::Active,
                    ..
                }
            ) && record.allocation.request.revision_id == target.revision_id
                && record.allocation.generation < target.manifest_generation
        })
    }) {
        return Err(CoreError::NotReady(
            "source activation would overtake an unresolved older publication for the same pair"
                .into(),
        ));
    }
    let Some(event) = event else {
        if state.histories.get(&target.repo_id).is_some_and(|history| {
            history.records.values().any(|record| {
                record.allocation.request.revision_id == target.revision_id
                    && record.allocation.generation == target.manifest_generation
            })
        }) {
            return Err(corrupt("reserved source target has no sealed event"));
        }
        return Ok(());
    };
    let record = existing(state, &target.repo_id, event)?
        .ok_or_else(|| corrupt("sealed source event was not durably reserved"))?;
    let (binding, phase) = bound(&record)?;
    if binding.target != *target {
        return Err(corrupt(
            "sealed source event target differs from its original reservation",
        ));
    }
    if phase != SourceEventPhaseV1::Staged {
        return Err(CoreError::NotReady("source activation requires its original committed stage receipt and a pending stream slot".into()));
    }
    let history = state
        .histories
        .get_mut(&target.repo_id)
        .ok_or_else(|| corrupt("missing source history"))?;
    let stream = history
        .streams
        .get_mut(&event.stream_id)
        .ok_or_else(|| corrupt("missing stream head"))?;
    if stream.pending.as_ref() != Some(&event.event_id)
        || stream.active != event.expected_base_event_id
    {
        return Err(conflict(
            "activation no longer owns the stream pending slot or base",
        ));
    }
    stream.pending = None;
    stream.active = Some(event.event_id.clone());
    let record = history
        .records
        .get_mut(&(event.stream_id.clone(), event.event_id.clone()))
        .ok_or_else(|| corrupt("missing source event"))?;
    if let SourceEventStateV1::Bound { phase, .. } = &mut record.state {
        *phase = SourceEventPhaseV1::Active;
    }
    Ok(())
}

impl ActivationCatalog {
    /// Restart and administrative rollback may serve only source publications
    /// that were actually accepted by a prior atomic paired activation. Merely
    /// sealing a stage does not establish source freshness authority.
    pub(crate) fn validate_proved_source_history(
        &self,
        target: &GenerationSnapshot,
        event: Option<&SourcePublicationEvent>,
    ) -> Result<(), CoreError> {
        let entries = self
            .entries
            .read()
            .map_err(|error| corrupt(error.to_string()))?;
        self.ensure_durability_certain_v1()?;
        match event {
            Some(event) => {
                let record = existing(&entries, &target.repo_id, event)?.ok_or_else(|| {
                    corrupt("proved active source event is absent from retained history")
                })?;
                let (binding, phase) = bound(&record)?;
                if phase != SourceEventPhaseV1::Active || binding.target != *target {
                    return Err(corrupt(
                        "proved source generation was never accepted by paired activation",
                    ));
                }
            }
            None => {
                if entries
                    .histories
                    .get(&target.repo_id)
                    .is_some_and(|history| {
                        history.records.values().any(|record| {
                            record.allocation.request.revision_id == target.revision_id
                                && record.allocation.generation == target.manifest_generation
                        })
                    })
                {
                    return Err(corrupt(
                        "proved historical source target lost its sealed event",
                    ));
                }
            }
        }
        Ok(())
    }
}
