//! The identity and digest accessors every receipt-bearing ingest batch
//! exposes (QI-BB-032), so one digest computation and one idempotency key
//! serve every route.
//!
//! The digest itself is fixed by the contract
//! ([`quanta_index_contract::INGEST_BATCH_DIGEST_DOMAIN_V1`]) and computed
//! by the codec owner (`quanta-index-ipc`), which hashes the body with the
//! digest field cleared: `batch_digest_mut` exists for that computation
//! only, so the field can be taken out, the body encoded, and the field
//! put back.

use quanta_index_contract::{
    DirtyIngestBatch, FileContributorIngestBatch, FileOwnershipIngestBatch, HistoryIngestBatch,
    IngestOperationKindV1, ManifestGeneration, RepoCommitRecencyIngestBatch,
    RepoDescriptionIngestBatch, RepoId, RepoMetaIngestBatch, RepoTopicIngestBatch, RevisionId,
    RuntimeCatalogIngestBatch, SearchCorpusIngestBatch, StructuralIngestBatch,
};

/// The identity every receipt-bearing ingest batch carries and the digest
/// that binds it to its body. The codec owner adds its own encoding bound
/// when it hashes the body; this trait names no wire format.
pub trait IngestBatchBodyV1 {
    /// The route this batch travels; part of its digest domain and its
    /// idempotency key.
    const OPERATION: IngestOperationKindV1;

    fn repo_id(&self) -> &RepoId;
    fn revision_id(&self) -> &RevisionId;
    fn generation(&self) -> ManifestGeneration;
    /// The digest the batch carries; the search plane recomputes it from
    /// the body and refuses the batch when they differ.
    fn batch_digest(&self) -> &str;
    fn batch_digest_mut(&mut self) -> &mut String;
}

macro_rules! ingest_batch_body_v1 {
    ($batch:ty, $operation:ident) => {
        impl IngestBatchBodyV1 for $batch {
            const OPERATION: IngestOperationKindV1 = IngestOperationKindV1::$operation;

            fn repo_id(&self) -> &RepoId {
                &self.repo_id
            }

            fn revision_id(&self) -> &RevisionId {
                &self.revision_id
            }

            fn generation(&self) -> ManifestGeneration {
                self.generation
            }

            fn batch_digest(&self) -> &str {
                &self.batch_digest
            }

            fn batch_digest_mut(&mut self) -> &mut String {
                &mut self.batch_digest
            }
        }
    };
}

ingest_batch_body_v1!(SearchCorpusIngestBatch, SearchCorpus);
ingest_batch_body_v1!(HistoryIngestBatch, History);
ingest_batch_body_v1!(DirtyIngestBatch, Dirty);
ingest_batch_body_v1!(RuntimeCatalogIngestBatch, RuntimeCatalog);
ingest_batch_body_v1!(StructuralIngestBatch, Structural);
ingest_batch_body_v1!(RepoCommitRecencyIngestBatch, RepoCommitRecency);
ingest_batch_body_v1!(RepoTopicIngestBatch, RepoTopic);
ingest_batch_body_v1!(RepoDescriptionIngestBatch, RepoDescription);
ingest_batch_body_v1!(FileOwnershipIngestBatch, FileOwnership);
ingest_batch_body_v1!(FileContributorIngestBatch, FileContributor);
ingest_batch_body_v1!(RepoMetaIngestBatch, RepoMeta);
