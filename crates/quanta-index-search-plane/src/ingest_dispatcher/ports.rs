//! The ingest-side owner ports this crate defines (history, runtime metadata,
//! structural) and the search-corpus authority write port.

use quanta_index_contract::{
    BatchPublishReceipt, DirtyIngestBatch, HistoryIngestBatch, ManifestGeneration, RepoId,
    RevisionId, RuntimeCatalogIngestBatch, StructuralIngestBatch,
};
use quanta_index_core::CoreError;

use crate::readiness::SearchCorpusHistoryRetentionReceiptV1;
use crate::{AuxiliaryAuthorityStore, SealedSearchCorpusAuthorityStateV1};

pub trait HistoryIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &HistoryIngestBatch) -> Result<BatchPublishReceipt, CoreError>;
}

pub trait RuntimeMetadataIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &DirtyIngestBatch) -> Result<BatchPublishReceipt, CoreError>;

    fn publish_catalog_batch(
        &self,
        batch: &RuntimeCatalogIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError>;
}

pub trait StructuralIngestPort: Send + Sync {
    fn publish_batch(
        &self,
        batch: &StructuralIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError>;
}

/// The read side of the durable sealed search-corpus history: whether one
/// exact `(generation, digest)` is recorded right now.
///
/// Activation and rollback consult it under the pair guard, immediately
/// before their durable CAS, so a candidate a concurrent seal has just
/// reaped from the authority is refused rather than activated on the
/// strength of an in-memory ledger that has not yet seen the receipt.
pub trait SearchCorpusAuthorityInspectPort: Send + Sync {
    fn inspect_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SealedSearchCorpusAuthorityStateV1, CoreError>;
}

/// Durable owner for complete lexical+semantic rollback history.
///
/// The port is intentionally composite. Per-track materializers cannot mint a
/// rollback target independently.
pub trait SearchCorpusAuthorityWritePort: SearchCorpusAuthorityInspectPort {
    /// Persist and reconcile the complete retained set.
    ///
    /// Contract: typed/contract errors reject before durable mutation.
    /// `CoreError::Storage` may describe a post-mutation durability ambiguity,
    /// so callers must keep rollback fenced until a later receipt succeeds.
    fn record_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError>;
}

impl SearchCorpusAuthorityInspectPort for AuxiliaryAuthorityStore {
    fn inspect_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SealedSearchCorpusAuthorityStateV1, CoreError> {
        Self::inspect_sealed_search_corpus(self, repo_id, revision_id, generation, manifest_digest)
    }
}

impl SearchCorpusAuthorityWritePort for AuxiliaryAuthorityStore {
    fn record_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError> {
        Self::record_sealed_search_corpus(self, repo_id, revision_id, generation, manifest_digest)
    }
}
