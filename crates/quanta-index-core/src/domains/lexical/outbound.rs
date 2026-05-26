use quanta_index_contract::{
    BatchPublishReceipt, LexicalCandidate, LexicalChannelOp, LexicalIngestBatch, LqQuery,
    ManifestGeneration, RepoId, RevisionId, SymbolCandidate,
};

use crate::error::CoreError;

/// Lexical readiness for a given generation. Reported by the lexical module to
/// the hybrid orchestrator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LexicalReadiness {
    pub manifest_generation: ManifestGeneration,
    pub materialized: bool,
}

/// Build / replay events into a lexical index for a given generation.
///
/// The build port consumes a (logical) batch of channel ops belonging to the
/// same `(repo, revision, generation)` triple and produces an opened index. The
/// adapter is responsible for any vendor-specific persistence (Tantivy, etc.).
pub trait LexicalIndexBuildPort: Send + Sync {
    fn build(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        ops: &[LexicalChannelOp],
    ) -> Result<(), CoreError>;
}

/// Open an existing lexical index for query.
pub trait LexicalIndexOpenPort: Send + Sync {
    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError>;
}

/// Ingest a typed lexical batch into the channel.
///
/// QI-RT-01 splits the producer-facing surface from the channel adapter so the
/// SDK can drop its direct dependency on `quanta-index-channel`.
/// Implementations fan the batch out to one or more [`LexicalChannelOp`]
/// writes and return the inclusive sequence range in
/// [`BatchPublishReceipt`].
pub trait LexicalIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &LexicalIngestBatch) -> Result<BatchPublishReceipt, CoreError>;
}

/// Searcher handle returned by [`LexicalIndexOpenPort::open`].
///
/// One per opened generation. Searcher is non-Send for performance (some
/// vendor handles are thread-local); the lexical module is responsible for
/// cache management.
pub trait LexicalSearcher: Send + Sync {
    fn search(&self, query: &LqQuery, top_k: u32) -> Result<Vec<LexicalCandidate>, CoreError>;

    /// Return symbol-domain matches only for the query within the opened
    /// generation. Implementations must not leak chunk docs through this
    /// surface.
    fn search_symbols(
        &self,
        query: &LqQuery,
        top_k: u32,
    ) -> Result<Vec<SymbolCandidate>, CoreError>;

    /// Return every lexical match for the query within the opened generation.
    /// Callers use this for exact scope materialization before downstream
    /// semantic/hybrid narrowing.
    fn search_all(&self, query: &LqQuery) -> Result<Vec<LexicalCandidate>, CoreError>;
}
