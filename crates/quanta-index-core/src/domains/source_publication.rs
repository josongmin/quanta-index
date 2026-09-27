//! Source-event identity survives generation reclamation. This port is backed
//! by the existing activation catalog; it does not authorize another registry.

use quanta_index_contract::{
    GenerationSnapshot, IngestOperationKindV1, RepoId, SearchPlaneTrackKind,
    SourcePublicationEvent, is_canonical_batch_digest_token_v1,
};

use crate::CoreError;
use crate::domains::idempotency::{IdempotencyCatalogPort, IdempotencyKeyV1};

/// The first admitted event's immutable target and original operation journal.
/// A replay under a newer target must resolve this original binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceEventBindingV1 {
    pub event: SourcePublicationEvent,
    pub target: GenerationSnapshot,
    pub journal_key: IdempotencyKeyV1,
}

impl SourceEventBindingV1 {
    pub fn validate(&self) -> Result<(), CoreError> {
        self.event
            .validate()
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        if self.target.track != SearchPlaneTrackKind::Lexical
            || self.journal_key.kind != IngestOperationKindV1::SearchCorpus
            || self.journal_key.repo_id != self.target.repo_id
            || self.journal_key.revision_id != self.target.revision_id
            || self.journal_key.generation != self.target.manifest_generation
            || !is_canonical_batch_digest_token_v1(&self.journal_key.batch_digest)
        {
            return Err(CoreError::InvalidContract(
                "source event binding must name its original lexical target and search-corpus journal key".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceEventPhaseV1 {
    /// Reserved before materialization; publication is not yet proven.
    Pending,
    /// The original journal records a committed stage receipt.
    Staged,
    /// A proved paired activation atomically advanced the stream head.
    Active,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceEventRecordV1 {
    pub binding: SourceEventBindingV1,
    pub phase: SourceEventPhaseV1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceEventReservationV1 {
    Reserved(SourceEventRecordV1),
    Existing(SourceEventRecordV1),
}

/// Durable source lineage in the existing activation catalog.
///
/// Keys are `(containing repo, stream_id, event_id)`, not target generations.
/// Only one pending/staged event may own a stream at a time. Its expected base
/// is compared with the active stream high-water, never with a staged target.
/// Neither a clock nor generation reclaim releases an unresolved reservation.
pub trait SourcePublicationCatalogPort: Send + Sync {
    /// Read the original binding. A reused event with a different payload or
    /// expected base is a conflict even when the requested generation is newer.
    fn inspect_source_event(
        &self,
        repo: &RepoId,
        event: &SourcePublicationEvent,
    ) -> Result<Option<SourceEventRecordV1>, CoreError>;

    /// Atomically compare stream lineage and persist the original journal key
    /// before either search track mutates. Existing returns the original target.
    fn reserve_source_event(
        &self,
        binding: &SourceEventBindingV1,
    ) -> Result<SourceEventReservationV1, CoreError>;

    /// Consult the actual original journal, never a caller-supplied PASS flag or
    /// receipt, to reconcile Pending into Staged. Missing, uncertain, in-flight
    /// or refused journal evidence is not a successful publication. No automatic
    /// release is permitted without a terminal nonpublication proof.
    fn reconcile_source_event(
        &self,
        repo: &RepoId,
        event: &SourcePublicationEvent,
        journal: &dyn IdempotencyCatalogPort,
    ) -> Result<SourceEventRecordV1, CoreError>;
}

#[cfg(test)]
mod tests {
    use super::{SourceEventBindingV1, SourcePublicationEvent};
    use crate::IdempotencyKeyV1;
    use quanta_index_contract::{
        GenerationSnapshot, IngestOperationKindV1, ManifestGeneration, RepoId, RevisionId,
        SearchPlaneTrackKind,
    };

    fn binding() -> SourceEventBindingV1 {
        let repo = RepoId::new("containing").expect("fixture repo");
        let revision = RevisionId::new("revision").expect("fixture revision");
        let generation = ManifestGeneration::new(1);
        SourceEventBindingV1 {
            event: SourcePublicationEvent {
                stream_id: "source-stream".into(),
                event_id: "event-1".into(),
                expected_base_event_id: None,
                payload_sha256: [1; 32],
            },
            target: GenerationSnapshot {
                repo_id: repo.clone(),
                revision_id: revision.clone(),
                manifest_generation: generation,
                manifest_digest: "manifest-1".into(),
                track: SearchPlaneTrackKind::Lexical,
            },
            journal_key: IdempotencyKeyV1 {
                kind: IngestOperationKindV1::SearchCorpus,
                repo_id: repo,
                revision_id: revision,
                generation,
                batch_digest: "a".repeat(64),
            },
        }
    }

    #[test]
    fn original_target_and_journal_binding_is_required() {
        let valid = binding();
        assert!(valid.validate().is_ok());
        let mutations: [fn(&mut SourceEventBindingV1); 6] = [
            |value| value.target.track = SearchPlaneTrackKind::Semantic,
            |value| value.journal_key.repo_id = RepoId::new("other").expect("fixture repo"),
            |value| {
                value.journal_key.revision_id = RevisionId::new("other").expect("fixture revision");
            },
            |value| value.journal_key.generation = ManifestGeneration::new(2),
            |value| value.journal_key.batch_digest = "not-canonical".into(),
            |value| value.event.expected_base_event_id = Some(value.event.event_id.clone()),
        ];
        for mutate in mutations {
            let mut invalid = valid.clone();
            mutate(&mut invalid);
            assert!(invalid.validate().is_err());
        }
    }
}
