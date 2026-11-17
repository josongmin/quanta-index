//! History input identity is independent of the current file SourceCut.
mod source_input;
mod snapshot_checkpoint;
use super::{Digest, IndexInstanceId, ObjectRef, SourceTransition};
use crate::{AuxEpochV1, ManifestGeneration, RepoId, RevisionId};
use sha2::{Digest as _, Sha256};
pub use source_input::{
    HistoryReplayFenceV1, HistoryReplayPublicationV1, HistorySourceInputV1,
    MAX_HISTORY_SOURCE_INPUT_BYTES_V1,
};
pub use snapshot_checkpoint::{
    HistorySnapshotCheckpointV1, HistorySnapshotExportFrameV1, HistorySnapshotPageV1, HistorySnapshotRowV1,
    MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1, MAX_HISTORY_SNAPSHOT_ROWS_PER_PAGE_V1,
};

/// The generation one auxiliary row belongs to. Core reexports this owner.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AuxiliaryGenerationKeyV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
}
crate::native_serde_struct!(AuxiliaryGenerationKeyV1 {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration
});

impl AuxiliaryGenerationKeyV1 {
    /// Canonical order for retained History FULL bootstrap within one repository.
    /// Different revisions can retain the same manifest generation ordinal.
    #[must_use]
    pub const fn bootstrap_order_key_v1(&self) -> (ManifestGeneration, &RevisionId) {
        (self.generation, &self.revision_id)
    }

    #[must_use]
    pub fn is_immediate_successor_v1(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> bool {
        &self.revision_id == revision && self.is_next_generation_of_repo_v1(repo, generation)
    }

    /// A new Source publication can carry a different revision of the same
    /// repository. Verify the exact predecessor cut and bind the new revision
    /// to the original batch independently.
    #[must_use]
    pub fn is_next_generation_of_repo_v1(
        &self,
        repo: &RepoId,
        generation: ManifestGeneration,
    ) -> bool {
        &self.repo_id == repo && self.generation.get().checked_add(1) == Some(generation.get())
    }
}

/// A write-settled complete History snapshot and its producer replay position.
/// Its source owner authenticates both under the same fence; decoding is not
/// a completeness proof. Core retains the canonical streaming row-set digest.
/// The row-set digest frames repo/revision/generation, then the epoch and
/// remaining rows in storage-key order as family code, key and stored CBOR.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistorySourceCutV1 {
    pub generation: AuxiliaryGenerationKeyV1,
    pub durable_epoch: AuxEpochV1,
    pub snapshot_digest: Digest,
    pub replay_start: Digest,
}
crate::native_serde_struct!(HistorySourceCutV1 {
    generation: AuxiliaryGenerationKeyV1,
    durable_epoch: AuxEpochV1,
    snapshot_digest: Digest,
    replay_start: Digest
});

impl HistorySourceCutV1 {
    /// Canonical member input context. This hash never grants a read or write.
    pub fn input_context_v1(&self) -> Result<Digest, &'static str> {
        let mut hash = Sha256::new();
        hash.update(b"qidx.native.history.input.v1\0");
        for identity in [
            self.generation.repo_id.as_str(),
            self.generation.revision_id.as_str(),
        ] {
            let length =
                u64::try_from(identity.len()).map_err(|_| "History identity exceeds u64")?;
            hash.update(length.to_le_bytes());
            hash.update(identity.as_bytes());
        }
        hash.update(self.generation.generation.get().to_le_bytes());
        hash.update(self.durable_epoch.get().to_le_bytes());
        hash.update(self.snapshot_digest.0);
        hash.update(self.replay_start.0);
        Ok(Digest(hash.finalize().into()))
    }
}

/// One sealed History input. None means FULL; Some names the exact prior cut.
/// The enclosing UpdateRequest still owns operation identity and view CAS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryTransition {
    pub instance: IndexInstanceId,
    pub expected: Option<HistorySourceCutV1>,
    pub next: HistorySourceCutV1,
    pub input: ObjectRef,
    /// Explicit target profile: initial FULL can add this required member in
    /// the same Catalog transition. It never publishes an empty placeholder.
    pub profile: ObjectRef,
}
crate::native_serde_struct!(HistoryTransition {
    instance: IndexInstanceId,
    expected: Option<HistorySourceCutV1>,
    next: HistorySourceCutV1,
    input: ObjectRef,
    profile: ObjectRef
});

/// Append one producer History batch to an existing required History member.
/// The input object contains the canonical HistoryIngestBatch JSON. Its
/// operation-keyed Published view is the durable commit receipt; no Legacy
/// ACK or caller-declared next cut is accepted here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryAppendV1 {
    pub instance: IndexInstanceId,
    pub expected: HistorySourceCutV1,
    pub input: ObjectRef,
    pub profile: ObjectRef,
}
crate::native_serde_struct!(HistoryAppendV1 {
    instance: IndexInstanceId,
    expected: HistorySourceCutV1,
    input: ObjectRef,
    profile: ObjectRef
});

/// Advance the History partition to the next manifest. The committed prior
/// instance supplies parent/ref closure and the original input supplies the
/// next generation's delta and manifest identity under the enclosing Source CAS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryGenerationAdvanceV1 {
    pub prior_instance: IndexInstanceId,
    pub expected: HistorySourceCutV1,
    pub instance: IndexInstanceId,
    pub input: ObjectRef,
    pub profile: ObjectRef,
}
crate::native_serde_struct!(HistoryGenerationAdvanceV1 {
    prior_instance: IndexInstanceId,
    expected: HistorySourceCutV1,
    instance: IndexInstanceId,
    input: ObjectRef,
    profile: ObjectRef
});

/// One immutable request and one Catalog publication for a Source advance and
/// its manifest's required History generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceHistoryTransitionV1 {
    pub source: SourceTransition,
    pub history: HistoryGenerationAdvanceV1,
}
crate::native_serde_struct!(SourceHistoryTransitionV1 {
    source: SourceTransition,
    history: HistoryGenerationAdvanceV1
});

/// Immutable source closure of a native History generation. Every present
/// child is the childless canonical JSON for its existing ingest DTO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryGenerationInputV1 {
    pub original: ObjectRef,
    pub repo_commit_recency: Option<ObjectRef>,
    pub file_contributor: Option<ObjectRef>,
}
crate::native_serde_struct!(HistoryGenerationInputV1 {
    original: ObjectRef,
    repo_commit_recency: Option<ObjectRef>,
    file_contributor: Option<ObjectRef>
});

impl HistoryGenerationInputV1 {
    pub fn child_references_v1(self) -> impl Iterator<Item = ObjectRef> {
        let mut edges = [Some(self.original), self.repo_commit_recency, self.file_contributor];
        edges.sort_unstable();
        edges.into_iter().flatten()
    }

    pub fn verify_children_v1(self, physical: &[ObjectRef]) -> Result<(), &'static str> {
        let mut prior = None;
        for edge in self.child_references_v1() {
            if edge.0 == Digest([0; 32]) || prior == Some(edge) {
                return Err("History generation input has an invalid or repeated child");
            }
            prior = Some(edge);
        }
        if !self.child_references_v1().eq(physical.iter().copied()) {
            return Err("History generation input child closure differs");
        }
        Ok(())
    }
}
