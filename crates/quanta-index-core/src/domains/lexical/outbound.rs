use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::{
    BatchPublishReceipt, CandidatePresenceV1, FileContributorIngestBatch, FileOwnerProjectionRow,
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

/// One page of lexical results plus what the adapter proved about the whole
/// match set.
///
/// Rows are always bounded by the caller's `top_k`; a `count` option does not
/// widen the page (QI-BB-005). What it does is make `exact_total` available:
/// the number of rows the query matches in this generation after constraints
/// and after any projection collapse that defines the row universe. Without a
/// `count` option the adapter fetches only the page plus its continuation
/// probe and reports `None`, and the caller derives an at-least window.
#[derive(Clone, Debug, PartialEq)]
pub struct LexicalSearchPageV1 {
    pub candidates: Vec<LexicalCandidate>,
    pub exact_total: Option<u64>,
}

/// What the lexical engine emits for one candidate under one plan
/// (QI-BB-022).
#[derive(Clone, Debug, PartialEq)]
pub enum LexicalCandidateExplanationV1 {
    /// No document with this id in the generation's index.
    NotIndexed,
    /// The document is indexed but the plan does not match it.
    NotMatched { reason: String },
    /// The document is indexed and matched; this is its emitted score.
    Matched(LexicalScoreTraceV1),
}

/// The score the lexical engine emits for one matched candidate, factored
/// into the engine's own score and the plan's boost.
///
/// `emitted_score == engine_score * boost_factor`; the search page carries
/// exactly `emitted_score` for the candidate.
#[derive(Clone, Debug, PartialEq)]
pub struct LexicalScoreTraceV1 {
    pub engine: LexicalScoreEngineV1,
    pub engine_score: f32,
    pub boost_factor: f32,
    pub emitted_score: f32,
}

/// Which scoring path produced a lexical engine score.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LexicalScoreEngineV1 {
    /// The inverted index's BM25 over the compiled plan.
    Bm25,
    /// An unindexed scan, where every match scores 1.
    UnindexedScan,
}

impl LexicalScoreEngineV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bm25 => "bm25",
            Self::UnindexedScan => "unindexed_scan",
        }
    }
}

/// Searcher handle returned by [`LexicalIndexOpenPort::open`].
///
/// One per opened sealed generation. The adapter performs a cold open and
/// proves the durable state; residency (which handles stay open, for how
/// long, under what budget) is the search plane's snapshot registry's job,
/// so a handle must be shareable across concurrent queries and must report
/// what it keeps resident.
pub trait LexicalSearcher: Send + Sync {
    /// Bytes this handle keeps resident while open: mapped index files plus
    /// decoded sidecars and metadata snapshots. An estimate taken at open
    /// time, used by the snapshot registry's byte budget; it must be
    /// monotone in the real cost so that a count-only cache cannot admit a
    /// corpus-sized handle as "one entry".
    fn resident_bytes_estimate(&self) -> u64;

    /// Unconstrained page of results; the constrained form is the one
    /// execution path.
    fn search(&self, query: &LqQuery, top_k: u32) -> Result<Vec<LexicalCandidate>, CoreError> {
        self.search_constrained(query, &QueryConstraintSetV1::unconstrained(), top_k)
            .map(|page| page.candidates)
    }

    /// Search with candidate-generation constraints applied before ranking and
    /// `top_k`. Adapters that do not own native constraint pushdown must fail
    /// closed for non-empty constraints instead of post-filtering results.
    fn search_constrained(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
    ) -> Result<LexicalSearchPageV1, CoreError>;

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
    ///
    /// Only structural routing uses this, to project exact chunk hits into
    /// structural buckets. It is unbounded by design and is the remaining
    /// full-recall collect that QI-BB-005's execution budget must cap (W4).
    /// Semantic scope narrowing does not use it: a lexical scope is a ranked,
    /// capped `search_constrained` (QI-BB-004).
    fn search_all(&self, query: &LqQuery) -> Result<Vec<LexicalCandidate>, CoreError>;

    /// Whether a candidate id is in this generation's index, by exact
    /// lookup (QI-BB-022). Never a ranked re-search, so the answer does not
    /// depend on the corpus around the candidate.
    fn candidate_presence(&self, candidate_id: &str) -> Result<CandidatePresenceV1, CoreError>;

    /// The score the engine emits for exactly one candidate under `query`
    /// and `constraints` (QI-BB-022), or why it emits none.
    ///
    /// Implementations must score the one document through the same
    /// compiled plan the ranked search uses, so a matched candidate's
    /// `emitted_score` is the score a page would carry for it.
    fn explain_candidate(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        candidate_id: &str,
    ) -> Result<LexicalCandidateExplanationV1, CoreError>;
}
