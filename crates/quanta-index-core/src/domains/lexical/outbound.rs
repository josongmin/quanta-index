use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::{
    BatchPublishReceipt, FileContributorIngestBatch, FileOwnerProjectionRow,
    FileOwnershipIngestBatch, LexicalCandidate, LqQuery, ManifestGeneration, QueryConstraintSetV1,
    RepoCommitRecencyIngestBatch, RepoDescriptionIngestBatch, RepoId, RepoMetaIngestBatch,
    RepoTopicIngestBatch, RevisionId, SearchCorpusIngestBatch, SymbolCandidate,
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

/// Build / replay a typed search-corpus ingest batch into a lexical index for a
/// given generation.
///
/// This is the batch-native authority surface used by the direct ingest path.
/// Implementations may internally lower into legacy op handlers, but callers
/// do not construct or route channel ops on the hot path.
pub trait SearchCorpusBatchBuildPort: Send + Sync {
    fn build_batch(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError>;
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

/// Ingest a typed search-corpus batch into the direct authority path.
///
/// QI-RT-01 splits the producer-facing surface from transport details so the
/// SDK can route batches over typed ingest IPC while runtime ownership stays
/// batch-native. Implementations commit the batch and return the accepted
/// surface in [`BatchPublishReceipt`].
pub trait SearchCorpusIngestPort: Send + Sync {
    fn publish_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError>;
}

/// Ingest a source-repo keyed commit-recency authority snapshot for one lexical
/// generation.
///
/// This authority powers repo-level history-backed gates on the lexical text
/// route (for example `repo:has.commit.after(...)`) without reusing the outer
/// `(repo_id, revision_id, generation)` history shard as if it were the
/// federated `source_repo_id` truth.
pub trait RepoCommitRecencyIngestPort: Send + Sync {
    fn publish_batch(
        &self,
        batch: &RepoCommitRecencyIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError>;
}

/// Ingest a source-repo keyed repo-metadata authority snapshot for one lexical
/// generation.
///
/// This authority powers repo-level metadata gates on the lexical text route
/// (for example `repo:has.meta(key:value)`). It is keyed by `source_repo_id`
/// like the commit-recency authority and is distinct from the per-generation
/// repo-metadata sidecar threaded into the searcher for presence checks.
pub trait RepoMetaIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &RepoMetaIngestBatch) -> Result<BatchPublishReceipt, CoreError>;
}

/// Ingest a source-repo keyed repo-topic authority snapshot for one lexical
/// generation.
///
/// This authority powers repo-level topic gates on the lexical text route
/// (for example `repo:has.topic(security)`). It is distinct from generic
/// repo metadata so topic support does not silently piggyback on unrelated
/// key/value substrate.
pub trait RepoTopicIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &RepoTopicIngestBatch)
    -> Result<BatchPublishReceipt, CoreError>;
}

/// Ingest a source-repo keyed repo-description authority snapshot for one
/// lexical generation.
///
/// This authority powers the regex-matched repo-description gate on the lexical
/// text route (`repo:has.description(<pattern>)`). It is distinct from generic
/// repo metadata and the repo-topic set: the description is a single free-text
/// string per `source_repo_id`, matched as a regex at query time, so it does
/// not silently piggyback on key/value or topic substrate.
pub trait RepoDescriptionIngestPort: Send + Sync {
    fn publish_batch(
        &self,
        batch: &RepoDescriptionIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError>;
}

/// Ingest a source-repo and repo-relative-path keyed file-ownership authority
/// snapshot for one lexical generation.
///
/// This authority powers file-level owner gates on the lexical text route
/// (for example `file:has.owner(@alice)`). Entries are query-facing owner
/// strings and the batch is treated as the complete ownership snapshot for the
/// addressed lexical generation.
pub trait FileOwnershipIngestPort: Send + Sync {
    fn publish_batch(
        &self,
        batch: &FileOwnershipIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError>;
}

/// Ingest a source-repo and repo-relative-path keyed file-contributor
/// authority snapshot for one lexical generation.
///
/// This authority powers file-level contributor gates on the lexical text
/// route (for example `file:has.contributor(alice)`). Entries are normalized
/// contributor identity strings and the batch is treated as the complete
/// contributor snapshot for the addressed lexical generation.
pub trait FileContributorIngestPort: Send + Sync {
    fn publish_batch(
        &self,
        batch: &FileContributorIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError>;
}

/// Searcher handle returned by [`LexicalIndexOpenPort::open`].
///
/// One per opened generation. Searcher is non-Send for performance (some
/// vendor handles are thread-local); the lexical module is responsible for
/// cache management.
pub trait LexicalSearcher: Send + Sync {
    fn search(&self, query: &LqQuery, top_k: u32) -> Result<Vec<LexicalCandidate>, CoreError>;

    /// Search with candidate-generation constraints applied before ranking and
    /// `top_k`. Adapters that do not own native constraint pushdown must fail
    /// closed for non-empty constraints instead of post-filtering results.
    fn search_constrained(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        if constraints.is_unconstrained() {
            self.search(query, top_k)
        } else {
            Err(CoreError::NotImplemented(
                "lexical searcher does not provide native query-constraint pushdown".to_string(),
            ))
        }
    }

    /// Project owner rows for the supplied lexical candidates.
    ///
    /// The input candidates already encode the lexical match set. Implementations
    /// must not widen it; they only attach source-repo/path keyed ownership rows.
    fn project_file_owners(
        &self,
        candidates: &[LexicalCandidate],
    ) -> Result<Vec<FileOwnerProjectionRow>, CoreError>;

    /// Return symbol-domain matches only for the query within the opened
    /// generation. Implementations must not leak chunk docs through this
    /// surface.
    fn search_symbols(
        &self,
        query: &LqQuery,
        top_k: u32,
    ) -> Result<Vec<SymbolCandidate>, CoreError>;

    fn search_symbols_constrained(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        if constraints.is_unconstrained() {
            self.search_symbols(query, top_k)
        } else {
            Err(CoreError::NotImplemented(
                "symbol searcher does not provide native query-constraint pushdown".to_string(),
            ))
        }
    }

    /// Return every symbol-domain match for the query within the opened
    /// generation. Callers use this when chunk-domain structural routing needs
    /// exact symbol-hit projection without top-k truncation.
    fn search_symbols_all(&self, query: &LqQuery) -> Result<Vec<SymbolCandidate>, CoreError> {
        self.search_symbols(query, u32::MAX)
    }

    /// Return every lexical match for the query within the opened generation.
    /// Callers use this for exact scope materialization before downstream
    /// semantic/hybrid narrowing.
    fn search_all(&self, query: &LqQuery) -> Result<Vec<LexicalCandidate>, CoreError>;

    fn search_all_constrained(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        if constraints.is_unconstrained() {
            self.search_all(query)
        } else {
            Err(CoreError::NotImplemented(
                "lexical searcher does not provide native unbounded query-constraint pushdown"
                    .to_string(),
            ))
        }
    }
}
