//! Non-persisted proof custody for one source-corpus publication.

use std::any::Any;

use quanta_index_contract::{
    ManifestGeneration, SearchCorpusIngestBatch, SearchPlaneTrackKind, SourcePublicationBinding,
};

use crate::CoreError;

/// One caller-owned publication scope, never a process or query cache.
///
/// This owner grants no validation by itself. An adapter may retain its private
/// validated proof type here only after proving the committed content. Every
/// subsequent use must authenticate the current bytes and inventory again.
/// Bodies, query handles and successful-response flags are not proof custody.
/// The owner is deliberately not Clone, serializable, or part of an IPC DTO.
#[derive(Default)]
pub struct PublicationValidationOwner {
    binding: Option<PublicationBodyBinding>,
    lexical_proof: Option<Box<dyn Any + Send + Sync>>,
    semantic_proof: Option<Box<dyn Any + Send + Sync>>,
}

#[derive(Eq, PartialEq)]
struct PublicationBodyBinding {
    publication: SourcePublicationBinding,
    base_generation: Option<ManifestGeneration>,
    seal: bool,
    payload_sha256: [u8; 32],
}

impl PublicationValidationOwner {
    /// Bind this scope before any physical validation or mutation. Reusing a
    /// publication owner for another body is a contract error, even when the
    /// two bodies happen to name the same generation.
    pub fn bind_batch(&mut self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        // The declared transport token alone cannot bind direct adapter calls.
        // Hash the actual mutation payload using its canonical contract owner;
        // containing target/base/seal fields are bound separately here.
        let binding = PublicationBodyBinding {
            publication: SourcePublicationBinding::for_batch(batch),
            base_generation: batch.base_generation,
            seal: batch.seal,
            payload_sha256: quanta_index_contract::source_event_payload_sha256(batch).map_err(
                |error| CoreError::InvalidContract(format!("publication validation body: {error}")),
            )?,
        };
        match &self.binding {
            Some(previous) if previous != &binding => Err(CoreError::InvalidContract(
                "publication validation owner belongs to another source-corpus batch".into(),
            )),
            Some(_) => Ok(()),
            None => {
                self.binding = Some(binding);
                Ok(())
            }
        }
    }

    /// Adapter-local, opaque proof custody. The adapter's concrete type and
    /// validated constructors remain outside core. Initializing its empty
    /// state conveys no authority; byte authentication is still mandatory.
    pub fn proof_state<T: Any + Default + Send + Sync>(
        &mut self,
        track: SearchPlaneTrackKind,
    ) -> Result<&mut T, CoreError> {
        let slot = match track {
            SearchPlaneTrackKind::Lexical => &mut self.lexical_proof,
            SearchPlaneTrackKind::Semantic => &mut self.semantic_proof,
            SearchPlaneTrackKind::Structural => {
                return Err(CoreError::InvalidContract(
                    "source-corpus publication proof custody has no structural track".into(),
                ));
            }
        };
        let proof = slot.get_or_insert_with(|| Box::<T>::default());
        proof.downcast_mut::<T>().ok_or_else(|| {
            CoreError::InvalidContract(
                "publication validation owner was used by a different proof producer".into(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::PublicationValidationOwner;
    use quanta_index_contract::{
        BatchIngestMode, ManifestGeneration, RepoId, RevisionId, SearchCorpusIngestBatch,
        SearchPlaneTrackKind, SourcePublicationEvent,
    };

    fn batch() -> SearchCorpusIngestBatch {
        SearchCorpusIngestBatch {
            source_event: SourcePublicationEvent {
                stream_id: "proof-stream".into(),
                event_id: "proof-event".into(),
                expected_base_event_id: None,
                payload_sha256: [0; 32],
            },
            repo_id: RepoId::new("repo").expect("fixture repo"),
            revision_id: RevisionId::new("revision").expect("fixture revision"),
            generation: ManifestGeneration::new(1),
            base_generation: None,
            manifest_digest: "proof-manifest".into(),
            batch_digest: "0".repeat(64),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: true,
        }
    }

    #[test]
    fn actual_body_changes_refuse_even_with_unchanged_declared_tokens() {
        let original = batch();
        for change in 0..4 {
            let mut owner = PublicationValidationOwner::default();
            owner.bind_batch(&original).expect("initial binding");
            owner.bind_batch(&original).expect("same body binding");
            let mut changed = original.clone();
            match change {
                0 => changed.bundle_payload = Some(b"different mutation bytes".to_vec()),
                1 => changed.base_generation = Some(ManifestGeneration::new(7)),
                2 => changed.seal = false,
                _ => changed.manifest_digest = "different-manifest".into(),
            }
            assert!(
                matches!(owner.bind_batch(&changed), Err(crate::CoreError::InvalidContract(message))
                if message.contains("belongs to another source-corpus batch"))
            );
        }
    }

    #[test]
    fn producer_substitution_refuses_without_erasing_a_track_proof() {
        let mut owner = PublicationValidationOwner::default();
        assert!(
            matches!(owner.proof_state::<u64>(SearchPlaneTrackKind::Structural),
            Err(crate::CoreError::InvalidContract(message)) if message.contains("no structural track"))
        );
        *owner
            .proof_state::<u64>(SearchPlaneTrackKind::Lexical)
            .expect("lexical state") = 7;
        assert!(
            matches!(owner.proof_state::<String>(SearchPlaneTrackKind::Lexical),
            Err(crate::CoreError::InvalidContract(message)) if message.contains("different proof producer"))
        );
        assert_eq!(
            *owner
                .proof_state::<u64>(SearchPlaneTrackKind::Lexical)
                .expect("retained state"),
            7
        );
        *owner
            .proof_state::<String>(SearchPlaneTrackKind::Semantic)
            .expect("separate semantic state") = "semantic".into();
        assert_eq!(
            *owner
                .proof_state::<u64>(SearchPlaneTrackKind::Lexical)
                .expect("independent lexical state"),
            7
        );
        assert_eq!(
            owner
                .proof_state::<String>(SearchPlaneTrackKind::Semantic)
                .expect("retained semantic state")
                .as_str(),
            "semantic"
        );
    }
}
