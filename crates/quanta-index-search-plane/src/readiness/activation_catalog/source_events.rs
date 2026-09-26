//! Source publication state is mutated only through the repository envelope.

use quanta_index_contract::{
    GenerationSnapshot, RepoId, SearchPlaneErrorCodeV2, SourcePublicationEvent,
};
use quanta_index_core::{
    CoreError, IdempotencyCatalogPort, OperationInspectV1, SourceEventBindingV1,
    SourceEventPhaseV1, SourceEventRecordV1, SourceEventReservationV1,
    SourcePublicationCatalogPort,
};

use super::repository_envelope::{MAX_EVENTS, MAX_STREAMS, StreamHead, capacity, corrupt};
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
    if let Some(record) = record {
        if record.binding.event != *event {
            return Err(conflict(
                "event identity reused with different payload or expected base",
            ));
        }
    }
    Ok(record.cloned())
}

impl SourcePublicationCatalogPort for ActivationCatalog {
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

    fn reserve_source_event(
        &self,
        binding: &SourceEventBindingV1,
    ) -> Result<SourceEventReservationV1, CoreError> {
        binding.validate()?;
        super::repository_envelope::bound_manifest_token(&binding.target.manifest_digest)?;
        let repo = &binding.target.repo_id;
        let mut entries = self
            .entries
            .write()
            .map_err(|error| corrupt(error.to_string()))?;
        self.ensure_durability_certain_v1()?;
        if let Some(record) = existing(&entries, repo, &binding.event)? {
            return Ok(SourceEventReservationV1::Existing(record));
        }
        let history = entries.histories.get(repo);
        let stream = history.and_then(|history| history.streams.get(&binding.event.stream_id));
        if stream.and_then(|stream| stream.pending.as_ref()).is_some() {
            return Err(CoreError::NotReady("source publication stream has an unresolved reservation; reconcile its original journal".into()));
        }
        if stream.and_then(|stream| stream.active.as_ref())
            != binding.event.expected_base_event_id.as_ref()
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
        if history.is_some_and(|history| {
            history.records.values().any(|record| {
                record.binding.target.repo_id == binding.target.repo_id
                    && record.binding.target.revision_id == binding.target.revision_id
                    && record.binding.target.manifest_generation
                        == binding.target.manifest_generation
            })
        }) {
            return Err(conflict(
                "target generation already belongs to another source event",
            ));
        }
        let mut next = entries.repository(repo);
        let history = next.histories.entry(repo.clone()).or_default();
        let stream = history
            .streams
            .entry(binding.event.stream_id.clone())
            .or_insert_with(StreamHead::default);
        stream.pending = Some(binding.event.event_id.clone());
        let record = SourceEventRecordV1 {
            binding: binding.clone(),
            phase: SourceEventPhaseV1::Pending,
        };
        let _prior = history.records.insert(
            (
                binding.event.stream_id.clone(),
                binding.event.event_id.clone(),
            ),
            record.clone(),
        );
        self.persist_repository(repo, &mut entries, next)?;
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
        // No catalog lock is held while calling another authority. Recheck the
        // original binding under the write lock before advancing its phase.
        let OperationInspectV1::Committed {
            receipt,
            durable_sequence,
        } = journal.inspect(&record.binding.journal_key)?
        else {
            return Err(CoreError::NotReady("source event original journal has no committed stage receipt; reservation retained".into()));
        };
        if receipt.generation != record.binding.target.manifest_generation
            || receipt.manifest_digest.as_ref() != Some(&record.binding.target.manifest_digest)
            || receipt.batch_digest != record.binding.journal_key.batch_digest
            || !receipt.sealed
            || receipt.semantic_content.is_none()
            || durable_sequence == 0
            || receipt.durable_sequence != durable_sequence
        {
            return Err(corrupt(
                "original committed journal receipt does not bind the sealed source target",
            ));
        }
        let mut entries = self
            .entries
            .write()
            .map_err(|error| corrupt(error.to_string()))?;
        self.ensure_durability_certain_v1()?;
        let observed = existing(&entries, repo, event)?
            .ok_or_else(|| corrupt("reservation disappeared during reconciliation"))?;
        if observed.binding != record.binding {
            return Err(corrupt(
                "original event binding changed during reconciliation",
            ));
        }
        if observed.phase != SourceEventPhaseV1::Pending {
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
        updated.phase = SourceEventPhaseV1::Staged;
        let updated = updated.clone();
        self.persist_repository(repo, &mut entries, next)?;
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
    let Some(event) = event else {
        if state.histories.get(&target.repo_id).is_some_and(|history| {
            history.records.values().any(|record| {
                record.binding.target.revision_id == target.revision_id
                    && record.binding.target.manifest_generation == target.manifest_generation
            })
        }) {
            return Err(corrupt("reserved source target has no sealed event"));
        }
        return Ok(());
    };
    let record = existing(state, &target.repo_id, event)?
        .ok_or_else(|| corrupt("sealed source event was not durably reserved"))?;
    if record.binding.target != *target {
        return Err(corrupt(
            "sealed source event target differs from its original reservation",
        ));
    }
    if record.phase != SourceEventPhaseV1::Staged {
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
    history
        .records
        .get_mut(&(event.stream_id.clone(), event.event_id.clone()))
        .ok_or_else(|| corrupt("missing source event"))?
        .phase = SourceEventPhaseV1::Active;
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
                if record.phase != SourceEventPhaseV1::Active || record.binding.target != *target {
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
                            record.binding.target.revision_id == target.revision_id
                                && record.binding.target.manifest_generation
                                    == target.manifest_generation
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
