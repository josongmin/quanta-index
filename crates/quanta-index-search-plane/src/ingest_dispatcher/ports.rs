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

/// Durable owner for complete lexical+semantic rollback history.
///
/// The port is intentionally composite. Per-track materializers cannot mint a
/// rollback target independently.
pub trait SearchCorpusAuthorityWritePort: Send + Sync {
    fn inspect_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SealedSearchCorpusAuthorityStateV1, CoreError>;

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

impl SearchCorpusAuthorityWritePort for AuxiliaryAuthorityStore {
    fn inspect_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SealedSearchCorpusAuthorityStateV1, CoreError> {
        Self::inspect_sealed_search_corpus(self, repo_id, revision_id, generation, manifest_digest)
    }

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
