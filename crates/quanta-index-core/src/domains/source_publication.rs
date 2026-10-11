//! Source-event identity survives generation reclamation. This port is backed
//! by the existing activation catalog; it does not authorize another registry.

use quanta_index_contract::{
    ACTIVATION_ROOT_INCARNATION_BYTES_V1, GenerationPin, GenerationSnapshot, IngestOperationKindV1,
    ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind, SourcePublicationEvent,
    is_canonical_batch_digest_token_v1,
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

/// The caller's logical event before its physical generation and final batch
/// digest exist.
///
/// The containing revision is part of this allocation intent;
/// the retained event key remains `(repo, stream_id, event_id)`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceEventAllocationRequestV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub event: SourcePublicationEvent,
}

impl SourceEventAllocationRequestV1 {
    pub fn validate(&self) -> Result<(), CoreError> {
        self.event
            .validate()
            .map_err(|error| CoreError::InvalidContract(error.to_string()))
    }
}

/// The original durable number. The incarnation fences a pre-restore
/// allocation from later acquiring a final journal binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceEventAllocationV1 {
    pub request: SourceEventAllocationRequestV1,
    pub generation: ManifestGeneration,
    pub root_incarnation: [u8; ACTIVATION_ROOT_INCARNATION_BYTES_V1],
}

impl SourceEventAllocationV1 {
    pub fn validate(&self) -> Result<(), CoreError> {
        self.request.validate()?;
        if self.generation.get() == 0 {
            return Err(CoreError::InvalidContract(
                "source event allocation generation must be nonzero".into(),
            ));
        }
        if self.root_incarnation == [0; ACTIVATION_ROOT_INCARNATION_BYTES_V1] {
            return Err(CoreError::InvalidContract(
                "source event allocation must name a nonzero root incarnation".into(),
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

/// One canonical retained event record. `Allocated` has no manifest digest or
/// journal key; `Bound` always has both and follows the existing phases.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceEventStateV1 {
    Allocated,
    Bound {
        binding: Box<SourceEventBindingV1>,
        phase: SourceEventPhaseV1,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceEventRecordV1 {
    pub allocation: SourceEventAllocationV1,
    pub state: SourceEventStateV1,
}

impl SourceEventRecordV1 {
    #[must_use]
    pub fn binding_v1(&self) -> Option<&SourceEventBindingV1> {
        match &self.state {
            SourceEventStateV1::Allocated => None,
            SourceEventStateV1::Bound { binding, .. } => Some(binding),
        }
    }

    #[must_use]
    pub fn phase_v1(&self) -> Option<SourceEventPhaseV1> {
        match &self.state {
            SourceEventStateV1::Allocated => None,
            SourceEventStateV1::Bound { phase, .. } => Some(*phase),
        }
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        self.allocation.validate()?;
        if let SourceEventStateV1::Bound { binding, .. } = &self.state {
            binding.validate()?;
            if binding.event != self.allocation.request.event
                || binding.target.repo_id != self.allocation.request.repo_id
                || binding.target.revision_id != self.allocation.request.revision_id
                || binding.target.manifest_generation != self.allocation.generation
            {
                return Err(CoreError::InvalidContract(
                    "source event final binding differs from its original allocation".into(),
                ));
            }
        }
        Ok(())
    }
}

/// A retry returns the same retained record, including an already-bound
/// original target. Neither variant implies successful publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceEventAllocationOutcomeV1 {
    Allocated(SourceEventRecordV1),
    Existing(SourceEventRecordV1),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceEventReservationV1 {
    Reserved(SourceEventRecordV1),
    Existing(SourceEventRecordV1),
}

/// Physical text documents and complete source files have distinct ID spaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LexicalDocumentNamespaceV1 {
    File,
    NativeText,
}

/// A durable, event-bound range; unused IDs are never returned to the counter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalDocumentVersionRangeV1 {
    pub first: u64,
    pub count: u64,
    pub scope_sha256: [u8; 32],
}

impl LexicalDocumentVersionRangeV1 {
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.scope_sha256 == [0; 32]
            || (self.count == 0 && self.first != 0)
            || (self.count != 0
                && (self.first == 0
                    || self
                        .first
                        .checked_add(self.count.saturating_sub(1))
                        .is_none()))
        {
            return Err(CoreError::InvalidContract(
                "invalid lexical document-version reservation".into(),
            ));
        }
        Ok(())
    }

    /// Infallible by construction: hashes fixed-width identity and validated repository bytes.
    #[must_use]
    pub fn scope_digest(
        incarnation: &[u8; ACTIVATION_ROOT_INCARNATION_BYTES_V1],
        repository: &RepoId,
        namespace: LexicalDocumentNamespaceV1,
    ) -> [u8; 32] {
        use sha2::{Digest as _, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"quanta-lexical-document-version-scope-v1\0");
        hash.update(incarnation);
        hash.update(repository.as_str().as_bytes());
        hash.update([match namespace {
            LexicalDocumentNamespaceV1::File => 1,
            LexicalDocumentNamespaceV1::NativeText => 2,
        }]);
        hash.finalize().into()
    }
}

/// Both ranges are persisted atomically in the existing source-event envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalDocumentVersionReservationV1 {
    pub allocation: SourceEventAllocationV1,
    pub file: LexicalDocumentVersionRangeV1,
    pub native_text: LexicalDocumentVersionRangeV1,
}

impl LexicalDocumentVersionReservationV1 {
    pub fn validate(&self) -> Result<(), CoreError> {
        self.allocation.validate()?;
        for (namespace, range) in [
            (LexicalDocumentNamespaceV1::File, &self.file),
            (LexicalDocumentNamespaceV1::NativeText, &self.native_text),
        ] {
            range.validate()?;
            if range.scope_sha256
                != LexicalDocumentVersionRangeV1::scope_digest(
                    &self.allocation.root_incarnation,
                    &self.allocation.request.repo_id,
                    namespace,
                )
            {
                return Err(CoreError::InvalidContract(
                    "lexical document-version range differs from its allocation scope".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Durable source lineage in the existing activation catalog.
///
/// Keys are `(containing repo, stream_id, event_id)`, not target generations.
/// Only one allocated/pending/staged event may own a stream at a time. Its expected base
/// is compared with the active stream high-water, never with a staged target.
/// Neither a clock nor generation reclaim releases an unresolved reservation.
pub trait SourcePublicationCatalogPort: Send + Sync {
    /// Reserve original component IDs after source-event binding, before writes.
    /// A retry must return identical ranges and reject different requested counts.
    /// Implementations must persist counter and binding in one durable mutation.
    fn reserve_lexical_document_versions(
        &self,
        _binding: &SourceEventBindingV1,
        _file_count: u64,
        _native_text_count: u64,
    ) -> Result<LexicalDocumentVersionReservationV1, CoreError> {
        Err(CoreError::InvalidContract(
            "source-publication catalog does not support durable lexical document IDs".into(),
        ))
    }

    /// Admit a new lexical overlay write only when this exact target has a
    /// retained allocation in the current root incarnation. This read does not
    /// allocate a number, bind a journal, or authorize sealed-target mutation.
    /// Missing ownership is retryable `NotReady`; terminal journal replay must
    /// be resolved before this admission is called.
    fn require_reserved_generation(&self, pin: &GenerationPin) -> Result<(), CoreError>;

    /// Read the original allocation or final binding. A reused event with a
    /// different payload or expected base is a conflict.
    fn inspect_source_event(
        &self,
        repo: &RepoId,
        event: &SourcePublicationEvent,
    ) -> Result<Option<SourceEventRecordV1>, CoreError>;

    /// Atomically allocate and durably persist a number before answering.
    /// The implementation owns trusted first-pair vacancy admission and the
    /// occupied-generation floor; callers cannot supply either as a hint.
    /// A retry returns the original record, including its final binding if
    /// the first attempt has since advanced. A different containing revision
    /// under the same event identity is a conflict.
    fn allocate_source_event(
        &self,
        request: &SourceEventAllocationRequestV1,
    ) -> Result<SourceEventAllocationOutcomeV1, CoreError>;

    /// Bind an allocated number to its validated final lexical target and
    /// canonical journal key exactly once, before either track mutates. The
    /// target must match the allocation's repo, revision and generation, and
    /// its root incarnation must still be current. A different body/key is a
    /// conflict. This also admits an explicitly numbered first publication:
    /// it must claim its target and advance the durable allocation high-water
    /// under the same catalog mutation. Existing bound-event replay continues
    /// to return the original binding, even if a caller names a newer target.
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
    use super::{
        SourceEventAllocationRequestV1, SourceEventAllocationV1, SourceEventBindingV1,
        SourcePublicationEvent,
    };
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

    #[test]
    fn allocated_number_and_incarnation_must_be_nonzero() {
        let bound = binding();
        let mut allocation = SourceEventAllocationV1 {
            request: SourceEventAllocationRequestV1 {
                repo_id: bound.target.repo_id,
                revision_id: bound.target.revision_id,
                event: bound.event,
            },
            generation: ManifestGeneration::new(0),
            root_incarnation: [1; quanta_index_contract::ACTIVATION_ROOT_INCARNATION_BYTES_V1],
        };
        assert!(allocation.validate().is_err());
        allocation.generation = ManifestGeneration::new(1);
        allocation.root_incarnation =
            [0; quanta_index_contract::ACTIVATION_ROOT_INCARNATION_BYTES_V1];
        assert!(allocation.validate().is_err());
    }
}
